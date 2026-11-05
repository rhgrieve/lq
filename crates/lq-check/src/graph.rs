//! A port of `@shopify/theme-graph`, for `getReferences`/`getDependencies`.
//!
//! The walk is depth-first and sync. Upstream's interleaving affects only the order
//! of references within a module, which checks consume as sets. Any error anywhere
//! fails the whole graph, as a rejected `buildThemeGraph` does; `themeCheckRun` then
//! has no graph and references are empty.

use crate::context_utils::recursive_read_directory;
use crate::deps::Dependencies;
use crate::source::{to_source_code, Ast, SourceCode, Theme};
use crate::to_schema::ThemeSchema;
use crate::visitor::visit;
use lq_json::ast::{node_at_path, PathKey};
use lq_json::jsonc::parse_json;
use lq_json::{uri, JsonNode, Value};
use lq_syntax::{Markup, Node};
use lq_theme::config::Mode;
use lq_theme::divergence::{Divergence, Divergences};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceType {
    Direct,
    Indirect,
    Preset,
}

/// `Location`. Ranges are byte offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub uri: String,
    pub range: Option<(u32, u32)>,
}

/// `Reference`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub source: Location,
    pub target: Location,
    pub ty: ReferenceType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleKind {
    Layout,
    Section,
    Block,
    Snippet,
    LiquidTemplate,
    JsonTemplate,
    SectionGroup,
    Asset,
}

/// `ThemeModule`.
#[derive(Clone, Debug)]
pub struct ThemeModule {
    pub kind: ModuleKind,
    pub uri: String,
    pub dependencies: Vec<Reference>,
    pub references: Vec<Reference>,
    pub exists: Option<bool>,
}

/// `ThemeGraph`.
#[derive(Debug, Default)]
pub struct ThemeGraph {
    pub root_uri: String,
    pub entry_points: Vec<String>,
    /// `modules`: traversed modules, by uri.
    pub modules: HashMap<String, ThemeModule>,
}

impl ThemeGraph {
    pub fn references(&self, u: &str) -> &[Reference] {
        self.modules.get(u).map_or(&[], |m| &m.references)
    }

    pub fn dependencies(&self, u: &str) -> &[Reference] {
        self.modules.get(u).map_or(&[], |m| &m.dependencies)
    }
}

/// A rejected promise: the graph is off.
type R<T = ()> = Result<T, String>;

struct Builder<'a, 'b> {
    root_uri: &'b str,
    deps: &'a dyn Dependencies<'a>,
    mode: Mode,
    sources: HashMap<&'b str, &'a SourceCode<'a>>,
    cache: HashMap<String, ThemeModule>,
    traversed: HashSet<String>,
    block_names: Option<Vec<String>>,
    /// [`Divergence::GraphNoType`].
    skip_no_type: bool,
}

const IMAGE_EXTENSIONS: [&str; 7] = ["jpg", "jpeg", "png", "gif", "webp", "heic", "ico"];

fn extname(u: &str) -> &str {
    u.rsplit('.').next().unwrap_or("")
}

fn str_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => lq_json::js::number_to_string(*n),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".into(),
        Value::Array(_) => String::new(),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// `Object.entries(x)`; `Err` when it throws.
fn entries(v: Option<&Value>) -> R<Vec<(String, &Value)>> {
    match v {
        None | Some(Value::Null) => Err("Cannot convert undefined or null to object".into()),
        Some(Value::Object(o)) => Ok(o.iter().map(|(k, v)| (k.to_string(), v)).collect()),
        Some(Value::Array(a)) => Ok(a.iter().enumerate().map(|(i, v)| (i.to_string(), v)).collect()),
        Some(Value::String(s)) if !s.is_empty() => Err("unexpected string".into()),
        Some(_) => Ok(Vec::new()),
    }
}

/// `node.children.find((child) => child.key.value === 'type')`. `Err` on a node that
/// `.children` or `.key` can't be read from.
fn find_type_property<'n, 's>(node: Option<&'n JsonNode<'s>>) -> R<Option<&'n JsonNode<'s>>> {
    match node {
        Some(JsonNode::Object { children, .. }) => {
            Ok(children.iter().find(|c| c.property_key() == Some("type")))
        }
        Some(JsonNode::Array { children, .. }) if children.is_empty() => Ok(None),
        _ => Err("Cannot read properties of undefined (reading 'children')".into()),
    }
}

fn range_of(node: &JsonNode<'_>, offset: u32) -> (u32, u32) {
    let l = node.loc();
    (l.start + offset, l.end + offset)
}

fn path_keys(path: &[String]) -> Vec<&str> {
    path.iter().map(String::as_str).collect()
}

impl<'a> Builder<'a, '_> {
    fn module(&mut self, kind: ModuleKind, u: String) -> String {
        self.cache.entry(u.clone()).or_insert_with(|| ThemeModule {
            kind,
            uri: u.clone(),
            dependencies: Vec::new(),
            references: Vec::new(),
            exists: None,
        });
        u
    }

    fn block_module(&mut self, name: &str) -> String {
        let u = uri::join(self.root_uri, &["blocks", &format!("{name}.liquid")]);
        self.module(ModuleKind::Block, u)
    }

    fn section_module(&mut self, name: &str) -> String {
        let u = uri::join(self.root_uri, &["sections", &format!("{name}.liquid")]);
        self.module(ModuleKind::Section, u)
    }

    fn section_group_module(&mut self, name: &str) -> String {
        let u = uri::join(self.root_uri, &["sections", &format!("{name}.json")]);
        self.module(ModuleKind::SectionGroup, u)
    }

    fn snippet_module(&mut self, name: &str) -> String {
        let u = uri::join(self.root_uri, &["snippets", &format!("{name}.liquid")]);
        self.module(ModuleKind::Snippet, u)
    }

    fn layout_module(&mut self, name: &str) -> String {
        let u = uri::join(self.root_uri, &["layout", &format!("{name}.liquid")]);
        self.module(ModuleKind::Layout, u)
    }

    fn asset_module(&mut self, asset: &str) -> Option<String> {
        let ext = extname(asset);
        if !(IMAGE_EXTENSIONS.contains(&ext) || matches!(ext, "js" | "css" | "svg")) {
            return None;
        }
        let u = uri::join(self.root_uri, &["assets", asset]);
        Some(self.module(ModuleKind::Asset, u))
    }

    /// `getModule(themeGraph, uri)`.
    fn get_module(&mut self, u: &str) -> R<Option<String>> {
        if self.cache.contains_key(u) {
            return Ok(Some(u.to_string()));
        }
        let rel = uri::relative(u, self.root_uri);
        Ok(if rel.starts_with("assets") {
            self.asset_module(&uri::basename(u, None))
        } else if rel.starts_with("blocks") {
            Some(self.block_module(&uri::basename(u, Some(".liquid"))))
        } else if rel.starts_with("layout") {
            Some(self.layout_module(&uri::basename(u, Some(".liquid"))))
        } else if rel.starts_with("sections") {
            Some(if rel.ends_with(".json") {
                self.section_group_module(&uri::basename(u, Some(".json")))
            } else {
                self.section_module(&uri::basename(u, Some(".liquid")))
            })
        } else if rel.starts_with("snippets") {
            Some(self.snippet_module(&uri::basename(u, Some(".liquid"))))
        } else if rel.starts_with("templates") {
            match extname(u) {
                "json" => Some(self.module(ModuleKind::JsonTemplate, u.to_string())),
                "liquid" => Some(self.module(ModuleKind::LiquidTemplate, u.to_string())),
                _ => return Err(format!("Unknown template type for {u}")),
            }
        } else {
            None
        })
    }

    fn bind(&mut self, source: &str, target: &str, range: Option<(u32, u32)>, ty: ReferenceType) {
        let r = Reference {
            source: Location { uri: source.to_string(), range },
            target: Location { uri: target.to_string(), range: None },
            ty,
        };
        if let Some(m) = self.cache.get_mut(source) {
            m.dependencies.push(r.clone());
        }
        if let Some(m) = self.cache.get_mut(target) {
            m.references.push(r);
        }
    }

    fn traverse(&mut self, u: &str) -> R {
        if !self.traversed.insert(u.to_string()) {
            return Ok(());
        }
        let exists = self.deps.fs().stat(u).is_ok();
        let Some(module) = self.cache.get_mut(u) else { return Ok(()) };
        module.exists = Some(exists);
        let kind = module.kind;
        if !exists {
            return Ok(());
        }
        match kind {
            ModuleKind::Asset => Ok(()),
            ModuleKind::JsonTemplate | ModuleKind::SectionGroup => self.traverse_json(u, kind),
            _ => self.traverse_liquid(u, kind),
        }
    }

    /// `getSourceCode(uri)`, preferring what the run already parsed.
    fn with_source<T>(&mut self, u: &str, f: impl FnOnce(&mut Self, &SourceCode<'_>) -> R<T>) -> R<T> {
        if let Some(&file) = self.sources.get(u) {
            return f(self, file);
        }
        let text = self.deps.fs().read_file(u)?;
        let file = to_source_code(u, &text);
        f(self, &file)
    }

    fn traverse_liquid(&mut self, u: &str, kind: ModuleKind) -> R {
        let targets = self.with_source(u, |this, file| {
            let Ast::Liquid(Ok(doc)) = &file.ast else { return Ok(None) };
            Ok(Some(this.liquid_references(&doc.root)))
        })?;
        let Some(targets) = targets else { return Ok(()) };
        let mut unique = Vec::new();
        for (target, range) in targets {
            self.bind(u, &target, Some(range), ReferenceType::Direct);
            if !unique.contains(&target) {
                unique.push(target);
            }
        }
        for t in unique {
            self.traverse(&t)?;
        }
        if kind == ModuleKind::Section {
            let name = uri::basename(u, Some(".liquid"));
            if let Some(schema) = self.deps.section_schema(&name) {
                self.traverse_schema(schema, u, kind)?;
            }
        } else if kind == ModuleKind::Block && self.mode == Mode::Theme {
            let name = uri::basename(u, Some(".liquid"));
            if let Some(schema) = self.deps.block_schema(&name) {
                self.traverse_schema(schema, u, kind)?;
            }
        }
        Ok(())
    }

    fn liquid_references<'n>(&mut self, root: &'n Node<'n>) -> Vec<(String, (u32, u32))> {
        let mut out = Vec::new();
        let mut this = self;
        visit(root, &mut |node, ancestors| {
            let parent = ancestors.last().copied();
            match node {
                Node::LiquidFilter(f) => {
                    if !matches!(f.name, "asset_url" | "asset_img_url" | "inline_asset_content") {
                        return;
                    }
                    let Some(Node::LiquidVariable(v)) = parent else { return };
                    let Node::String(expr) = &v.expression else { return };
                    if !v.filters.first().is_some_and(|x| std::ptr::eq(x, node)) {
                        return;
                    }
                    if let Some(m) = this.asset_module(expr.value) {
                        let p = parent.unwrap().position();
                        out.push((m, (p.start, p.end)));
                    }
                }
                Node::ContentForMarkup(m) => {
                    let Node::String(ty) = &m.content_for_type else { return };
                    if ty.value != "block" {
                        return;
                    }
                    let Some(arg) = m.args.iter().find_map(|a| match a {
                        Node::NamedArgument(n) if n.name == "type" => Some(n),
                        _ => None,
                    }) else {
                        return;
                    };
                    let Node::String(block) = &arg.value else { return };
                    let target = this.block_module(block.value);
                    out.push((target, (parent.map_or(0, |p| p.position().start), node.position().end)));
                }
                Node::BlockMarkup(m) => {
                    let Node::String(name) = &m.name else { return };
                    if name.value.starts_with("shopify://apps/") {
                        return;
                    }
                    let target = this.block_module(name.value);
                    let p = parent.map_or(node.position(), Node::position);
                    out.push((target, (p.start, p.end)));
                }
                Node::RenderMarkup(m) => {
                    if let Node::String(s) = &m.snippet {
                        let target = this.snippet_module(s.value);
                        let p = parent.map_or(node.position(), Node::position);
                        out.push((target, (p.start, p.end)));
                    }
                }
                Node::LiquidTag(tag) => {
                    let p = node.position();
                    if tag.name == "sections" {
                        if let Markup::Node(m) = &tag.markup {
                            if let Node::String(s) = &**m {
                                let target = this.section_group_module(s.value);
                                out.push((target, (p.start, p.end)));
                            }
                        }
                    } else if tag.name == "section" {
                        if let Markup::Node(m) = &tag.markup {
                            if let Node::SectionMarkup(sm) = &**m {
                                if let Node::String(s) = &sm.name {
                                    let target = this.section_module(s.value);
                                    out.push((target, (p.start, p.end)));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        });
        let _ = &mut this;
        out
    }

    fn accepts_local_blocks(&self, section_type: &str) -> bool {
        let Some(schema) = self.deps.section_schema(section_type) else { return true };
        match schema.valid_schema() {
            Some(Ok(Some(v))) => v
                .get("blocks")
                .and_then(Value::as_array)
                .is_some_and(|blocks| {
                    blocks.iter().any(|b| {
                        b.get("type").is_some_and(Value::is_truthy) && b.get("name").is_some_and(Value::is_truthy)
                    })
                }),
            Some(Ok(None)) => false,
            _ => true,
        }
    }

    fn traverse_schema(&mut self, schema: &'a ThemeSchema<'a>, u: &str, kind: ModuleKind) -> R {
        if kind == ModuleKind::Section && self.accepts_local_blocks(&uri::basename(u, Some(".liquid"))) {
            return Ok(());
        }
        let Some(Ok(Some(valid))) = schema.valid_schema() else { return Ok(()) };
        let Ok(ast) = &schema.ast else { return Ok(()) };
        let offset = schema.offset;
        if let Some(blocks) = valid.get("blocks").filter(|b| b.is_truthy()) {
            for (i, block) in entries(Some(blocks))? {
                let path = ["blocks".to_string(), i];
                let Some(prop) = find_type_property(node_at_path(ast, &path_keys(&path)))? else { continue };
                let range = range_of(prop, offset);
                let ty = block.get("type").and_then(Value::as_str).unwrap_or("");
                match ty {
                    "@theme" => {
                        let names = self.theme_block_names()?;
                        for name in names.iter().filter(|n| !n.starts_with('_')) {
                            let m = self.block_module(name);
                            self.bind(u, &m, Some(range), ReferenceType::Indirect);
                            self.traverse(&m)?;
                        }
                    }
                    "@app" => {}
                    _ => {
                        let m = self.block_module(&str_of(block.get("type").unwrap_or(&Value::Null)));
                        self.bind(u, &m, Some(range), ReferenceType::Direct);
                        self.traverse(&m)?;
                    }
                }
            }
        }
        if let Some(presets) = valid.get("presets").filter(|p| p.is_truthy()) {
            for (i, preset) in entries(Some(presets))? {
                let Some(blocks) = preset.get("blocks") else { continue };
                let path = vec!["presets".to_string(), i];
                self.traverse_preset_blocks(u, ast, blocks, &path, offset)?;
            }
        }
        if let Some(default) = valid.get("default").filter(|d| d.is_truthy()) {
            if let Some(blocks) = default.get("blocks") {
                for (key, block) in entries(Some(blocks))? {
                    let path = ["default".to_string(), "blocks".to_string(), key];
                    let node = node_at_path(ast, &path_keys(&path));
                    let m = self.block_module(&str_of(block.get("type").unwrap_or(&Value::Null)));
                    let Some(prop) = find_type_property(node)? else { continue };
                    self.bind(u, &m, Some(range_of(prop, offset)), ReferenceType::Preset);
                    self.traverse(&m)?;
                }
            }
        }
        Ok(())
    }

    fn traverse_preset_blocks(&mut self, u: &str, ast: &JsonNode<'_>, blocks: &Value, parent: &[String], offset: u32) -> R {
        for (key, block) in entries(Some(blocks))? {
            let mut path = parent.to_vec();
            path.extend(["blocks".to_string(), key]);
            let node = node_at_path(ast, &path_keys(&path));
            let m = self.block_module(&str_of(block.get("type").unwrap_or(&Value::Null)));
            let Some(prop) = find_type_property(node)? else { continue };
            self.bind(u, &m, Some(range_of(prop, offset)), ReferenceType::Preset);
            self.traverse(&m)?;
            if let Some(nested) = block.get("blocks").filter(|b| b.is_truthy()) {
                self.traverse_preset_blocks(u, ast, nested, &path, offset)?;
            }
        }
        Ok(())
    }

    fn theme_block_names(&mut self) -> R<Vec<String>> {
        if self.block_names.is_none() {
            let dir = uri::join(self.root_uri, &["blocks"]);
            let uris = recursive_read_directory(self.deps.fs(), &dir, &|u, _| u.ends_with(".liquid"))?;
            self.block_names = Some(uris.iter().map(|u| uri::basename(u, Some(".liquid"))).collect());
        }
        Ok(self.block_names.clone().unwrap_or_default())
    }

    fn traverse_json(&mut self, u: &str, kind: ModuleKind) -> R {
        self.with_source(u, |this, file| {
            let Ast::Json(ast) = &file.ast else { return Err("Unexpected code path encountered".into()) };
            let Ok(ast) = ast else { return Ok(()) };
            let parsed = parse_json(file.source).unwrap_or(Value::Null);
            for (key, section) in entries(parsed.get("sections"))? {
                let section_type = str_of(section.get("type").ok_or("undefined section type").or_else(|e| {
                    if matches!(section, Value::Object(_)) { Ok(&UNDEFINED) } else { Err(e.to_string()) }
                })?);
                let path = ["sections".to_string(), key];
                let node = node_at_path(ast, &path_keys(&path));
                // Context override templates legitimately have entries without a type. Upstream
                // creates a module named `undefined`, then throws on the missing `type` property.
                if this.skip_no_type && matches!(find_type_property(node), Ok(None)) {
                    continue;
                }
                let module = this.section_module(&section_type);
                let prop = find_type_property(node)?.ok_or("Cannot read properties of undefined (reading 'loc')")?;
                this.bind(u, &module, Some(range_of(prop, 0)), ReferenceType::Direct);
                this.traverse(&module)?;
                this.traverse_section_references(u, ast, &path, section, &section_type)?;
            }
            if kind == ModuleKind::JsonTemplate {
                let layout = parsed.get("layout");
                let name = match layout {
                    Some(Value::Bool(false)) => None,
                    None => Some("theme".to_string()),
                    Some(v) => Some(str_of(v)),
                };
                if let Some(name) = name {
                    let module = this.layout_module(&name);
                    let range = layout.and_then(|_| node_at_path(ast, &["layout"])).map(|n| range_of(n, 0));
                    this.bind(u, &module, range, ReferenceType::Indirect);
                    this.traverse(&module)?;
                }
            }
            Ok(())
        })
    }

    fn traverse_section_references(&mut self, source: &str, ast: &JsonNode<'_>, path: &[String], section: &Value, section_type: &str) -> R {
        let Some(blocks) = section.get("blocks").filter(|b| b.is_truthy()) else { return Ok(()) };
        if self.accepts_local_blocks(section_type) {
            return Ok(());
        }
        self.traverse_blocks(source, ast, path, blocks)
    }

    /// `traverseSectionReferences`' loop and `traverseBlockReferences`.
    fn traverse_blocks(&mut self, source: &str, ast: &JsonNode<'_>, parent: &[String], blocks: &Value) -> R {
        for (key, block) in entries(Some(blocks))? {
            let mut path = parent.to_vec();
            path.extend(["blocks".to_string(), key]);
            let node = node_at_path(ast, &path_keys(&path));
            if self.skip_no_type && matches!(find_type_property(node), Ok(None)) {
                continue;
            }
            let module = self.block_module(&str_of(block.get("type").unwrap_or(&UNDEFINED)));
            let prop = find_type_property(node)?.ok_or("Cannot read properties of undefined (reading 'loc')")?;
            self.bind(source, &module, Some(range_of(prop, 0)), ReferenceType::Direct);
            self.traverse(&module)?;
            if let Some(nested) = block.get("blocks").filter(|b| b.is_truthy()) {
                self.traverse_blocks(source, ast, &path, nested)?;
            }
        }
        Ok(())
    }
}

/// `${undefined}` in a path.
static UNDEFINED: Value = Value::Null;

// `PathKey` conversions take `&str`; keep the import used for docs of `node_at_path`.
#[allow(dead_code)]
fn _key(s: &str) -> PathKey<'_> {
    PathKey::Str(s)
}

/// `buildThemeGraph(rootUri, deps)`.
pub fn build_theme_graph<'a>(
    root_uri: &str,
    deps: &'a dyn Dependencies<'a>,
    theme: &'a Theme<'a>,
    mode: Mode,
    divergences: Divergences,
) -> Result<ThemeGraph, String> {
    let join = |d: &str| uri::join(root_uri, &[d]);
    let (templates, layout, sections, blocks) = (join("templates"), join("layout"), join("sections"), join("blocks"));
    let entry_points = recursive_read_directory(deps.fs(), root_uri, &|u, _| {
        u.starts_with(&templates)
            || (u.starts_with(&layout) && u.ends_with(".liquid"))
            || (u.starts_with(&sections) && u.ends_with(".liquid"))
            || (mode == Mode::App && u.starts_with(&blocks) && u.ends_with(".liquid"))
    })?;
    let mut b = Builder {
        root_uri,
        deps,
        mode,
        sources: theme.iter().map(|f| (f.uri.as_str(), f)).collect(),
        cache: HashMap::new(),
        traversed: HashSet::new(),
        block_names: None,
        skip_no_type: divergences.contains(Divergence::GraphNoType),
    };
    let mut entries_out = Vec::new();
    for u in &entry_points {
        if let Some(m) = b.get_module(u)? {
            entries_out.push(m);
        }
    }
    for m in &entries_out {
        b.traverse(m)?;
    }
    let traversed = std::mem::take(&mut b.traversed);
    let modules = b.cache.into_iter().filter(|(k, _)| traversed.contains(k)).collect();
    Ok(ThemeGraph { root_uri: root_uri.to_string(), entry_points: entries_out, modules })
}
