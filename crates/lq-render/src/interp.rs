//! The interpreter. It renders a page the way Shopify's renderer does, closely enough that the
//! profile tree has the same shape, and counts the operations each node performs.

use crate::cost::CostModel;
use crate::profile::{FrameKey, Profile};
use crate::theme::{Theme, ThemeFile};
use crate::value::{Object, Value};
use lq_syntax::{LiquidTag, Markup, Node, Position};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub(crate) type Scope = HashMap<Rc<str>, Value>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    Normal,
    Break,
    Continue,
}

/// Limits that keep runaway themes bounded.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_nodes: u64,
    pub max_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_nodes: 2_000_000, max_depth: 64 }
    }
}

/// The section or block whose `content_for 'blocks'` renders next.
#[derive(Clone)]
pub(crate) struct Container {
    pub section: Value,
    pub blocks: Vec<Value>,
}

pub struct Renderer<'t> {
    pub theme: &'t Theme,
    pub model: &'t CostModel,
    pub profile: Profile,
    pub globals: Scope,
    pub(crate) scopes: Vec<Scope>,
    counters: HashMap<Rc<str>, i64>,
    cycles: HashMap<String, usize>,
    db_seen: HashSet<(u32, Rc<str>)>,
    pub(crate) file: Option<&'t ThemeFile>,
    pub(crate) paginate: Vec<(String, Value)>,
    pub(crate) containers: Vec<Container>,
    pub(crate) translations: Value,
    /// `file:line` of a `for` tag → iterations, from calibration.
    pub loop_shapes: HashMap<String, usize>,
    pub errors: Vec<String>,
    loaded: HashSet<Rc<str>>,
    /// Filters and tags lq passed through without modelling them.
    pub unsupported: std::collections::BTreeSet<String>,
    pub layout: Option<String>,
    nodes: u64,
    depth: usize,
    limits: Limits,
    stopped: bool,
    in_liquid_tag: bool,
    ifchanged: HashMap<u32, String>,
}

impl<'t> Renderer<'t> {
    pub fn new(theme: &'t Theme, model: &'t CostModel, name: &str, limits: Limits) -> Renderer<'t> {
        let translations =
            theme.default_locale().map(|v| crate::value::from_json(&v)).unwrap_or_default();
        Renderer {
            theme,
            model,
            profile: Profile::new(name),
            globals: Scope::new(),
            scopes: vec![Scope::new()],
            counters: HashMap::new(),
            cycles: HashMap::new(),
            db_seen: HashSet::new(),
            file: None,
            paginate: Vec::new(),
            containers: Vec::new(),
            translations,
            loop_shapes: HashMap::new(),
            errors: Vec::new(),
            loaded: HashSet::new(),
            unsupported: Default::default(),
            layout: None,
            nodes: 0,
            depth: 0,
            limits,
            stopped: false,
            in_liquid_tag: false,
            ifchanged: HashMap::new(),
        }
    }

    pub fn count(&mut self, op: &str, n: f64) {
        self.profile.count(op, n);
    }

    pub fn set_global(&mut self, name: &str, value: Value) {
        self.globals.insert(Rc::from(name), value);
    }

    fn src(&self, pos: Position) -> &'t str {
        let f = self.file.expect("rendering inside a file");
        f.source.get(pos.start as usize..pos.end as usize).unwrap_or("")
    }

    /// The current template's name as Shopify reports it: the path without `.liquid`.
    fn file_path(&self) -> Rc<str> {
        match self.file {
            Some(f) => match f.path.strip_suffix(".liquid") {
                Some(name) => Rc::from(name),
                None => f.path.clone(),
            },
            None => Rc::from(""),
        }
    }

    /// Opens a frame with no line, as Shopify's wrapper frames are (`liquid_template`, `section`, ...).
    pub(crate) fn open_plain(&mut self, name: &str, file: &str) {
        self.profile.open(FrameKey { name: name.to_string(), file: Rc::from(file), line: 0, col: 0 });
    }

    pub(crate) fn current_template(&self) -> String {
        self.file_path().to_string()
    }

    /// Renders a template under the frames Shopify opens for it: `liquid_template`, plus
    /// `section` for section and block files.
    pub(crate) fn render_template(&mut self, path: &str, section_frame: bool, out: &mut String) -> bool {
        if self.theme.get(path).is_none() {
            return false;
        }
        let name = path.strip_suffix(".liquid").unwrap_or(path).to_string();
        self.open_plain("liquid_template", &name);
        self.count("template", 1.0);
        if section_frame {
            self.open_plain("section", &name);
            self.count("section", 1.0);
        }
        let ok = self.render_file(path, out);
        if section_frame {
            self.profile.close();
        }
        self.profile.close();
        ok
    }

    pub(crate) fn line_of(&self, offset: u32) -> u32 {
        self.file.map_or(0, |f| f.line_col(offset).0)
    }

    /// Prints a Liquid error inline, as Shopify does, and keeps rendering.
    pub(crate) fn error(&mut self, offset: u32, message: &str, out: &mut String) {
        let template = self.file_path().to_string();
        let line = self.line_of(offset);
        let text = format!("Liquid error ({template} line {line}): {message}");
        self.count("error", 1.0);
        out.push_str(&text);
        if self.errors.len() < 200 {
            self.errors.push(text);
        }
    }

    // ---- files -------------------------------------------------------------------------------

    /// Renders a whole file. The caller opens any frame around it.
    pub fn render_file(&mut self, path: &str, out: &mut String) -> bool {
        let Some(file) = self.theme.get(path) else { return false };
        // Shopify loads each template once per request; the first render of a file pays for it.
        if self.loaded.insert(file.path.clone()) {
            self.count("template_load", 1.0);
        }
        let doc = match file.doc() {
            Ok(d) => d,
            Err(msg) => {
                let msg = msg.to_string();
                let prev = self.file.replace(file);
                self.error(0, &format!("{path} could not be parsed: {msg}"), out);
                self.file = prev;
                return true;
            }
        };
        let prev = self.file.replace(file);
        let len = file.source.len() as u32;
        self.render_body(doc.children(), Position::new(0, len), false, false, out);
        self.file = prev;
        true
    }

    // ---- bodies ------------------------------------------------------------------------------

    /// Renders the Liquid nodes in `range`, printing the text between them. Text nodes
    /// themselves are skipped: their parsed value is trimmed, so the source is printed instead.
    pub(crate) fn render_body(
        &mut self,
        children: &'t [Node<'t>],
        range: Position,
        trim_first: bool,
        trim_last: bool,
        out: &mut String,
    ) -> Flow {
        let file = self.file.expect("file");
        let source = file.source;
        let mut cursor = range.start as usize;
        let mut trim_next = trim_first;
        for child in children {
            if matches!(child, Node::TextNode(_)) {
                continue;
            }
            let pos = child.position();
            if !self.in_liquid_tag {
                let mut gap = source.get(cursor..pos.start as usize).unwrap_or("");
                if trim_next {
                    gap = gap.trim_start();
                }
                if whitespace_start(child) {
                    gap = gap.trim_end();
                }
                out.push_str(gap);
            }
            let flow = self.render_node(child, out);
            cursor = pos.end as usize;
            trim_next = whitespace_after(child);
            if flow != Flow::Normal || self.stopped {
                return flow;
            }
        }
        if !self.in_liquid_tag {
            let end = (range.end as usize).max(cursor);
            let mut gap = source.get(cursor..end).unwrap_or("");
            if trim_next {
                gap = gap.trim_start();
            }
            if trim_last {
                gap = gap.trim_end();
            }
            out.push_str(gap);
        }
        Flow::Normal
    }

    fn render_node(&mut self, node: &'t Node<'t>, out: &mut String) -> Flow {
        self.nodes += 1;
        if self.nodes > self.limits.max_nodes {
            if !self.stopped {
                self.stopped = true;
                self.errors.push(format!(
                    "Stopped after {} nodes; raise the limit with --max-nodes",
                    self.limits.max_nodes
                ));
            }
            return Flow::Break;
        }
        match node {
            Node::LiquidTag(tag) if tag.name == "liquid" => {
                // The Liquid gem parses `{% liquid %}` lines into the enclosing body, so its tags
                // are profiled as siblings, not under a `liquid` frame.
                let Markup::List(lines) = &tag.markup else { return Flow::Normal };
                let prev = std::mem::replace(&mut self.in_liquid_tag, true);
                let mut flow = Flow::Normal;
                for line in lines {
                    flow = self.render_node(line, out);
                    if flow != Flow::Normal || self.stopped {
                        break;
                    }
                }
                self.in_liquid_tag = prev;
                flow
            }
            Node::LiquidTag(tag) => {
                // Inline comments aren't profiled.
                if tag.name == "#" || tag.name == "comment" {
                    return Flow::Normal;
                }
                let custom = self.open_tag_frames(tag.name, tag.position.start);
                self.count("node", 1.0);
                let flow = self.render_tag(tag, out);
                self.close_tag_frames(custom);
                flow
            }
            Node::LiquidVariableOutput(o) => {
                self.open_frame("Variable".to_string(), o.position.start);
                self.count("node", 1.0);
                let start = out.len();
                match &o.markup {
                    Markup::Node(v) => {
                        let v = self.eval(v);
                        v.render_into(out);
                    }
                    Markup::Str(s) => {
                        let msg = format!("Liquid syntax error: unexpected '{}'", s.trim());
                        self.error(o.position.start, &msg, out);
                    }
                    _ => {}
                }
                self.count("output_byte", (out.len() - start) as f64);
                self.profile.close();
                Flow::Normal
            }
            Node::LiquidRawTag(raw) => {
                if raw.name == "comment" {
                    return Flow::Normal;
                }
                let custom = self.open_tag_frames(raw.name, raw.position.start);
                self.count("node", 1.0);
                match raw.name {
                    "raw" => {
                        if let Node::RawMarkup(m) = &raw.body {
                            out.push_str(m.value);
                        }
                    }
                    "style" => {
                        if let Node::RawMarkup(m) = &raw.body {
                            out.push_str("<style data-shopify>");
                            let start = out.len();
                            self.render_body(&m.nodes, m.position, false, false, out);
                            self.count("output_byte", (out.len() - start) as f64);
                            out.push_str("</style>");
                        }
                    }
                    // schema, stylesheet and javascript compile to assets; doc prints nothing.
                    _ => {}
                }
                self.close_tag_frames(custom);
                Flow::Normal
            }
            Node::LiquidErrorNode(e) => {
                let msg = e.message.to_string();
                self.error(e.position.start, &msg, out);
                Flow::Normal
            }
            _ => Flow::Normal,
        }
    }

    /// Opens a tag's frame, named as the Liquid gem names it: the tag class (`Assign`, `For`,
    /// `Render`, ...) for standard tags, and Shopify's own tags by name under a `Custom` frame.
    /// Returns whether a `Custom` frame was opened.
    fn open_tag_frames(&mut self, tag: &str, offset: u32) -> bool {
        let class = match tag {
            "assign" => "Assign",
            "capture" => "Capture",
            "case" => "Case",
            "cycle" => "Cycle",
            "decrement" => "Decrement",
            "increment" => "Increment",
            "echo" => "Echo",
            "for" => "For",
            "if" => "If",
            "unless" => "Unless",
            "ifchanged" => "Ifchanged",
            "include" => "Include",
            "render" => "Render",
            "raw" => "Raw",
            "tablerow" => "TableRow",
            "break" => "Break",
            "continue" => "Continue",
            "doc" => "Doc",
            _ => {
                let file = self.file_path();
                self.open_plain("Custom", &file);
                self.open_frame(tag.to_string(), offset);
                return true;
            }
        };
        self.open_frame(class.to_string(), offset);
        false
    }

    fn close_tag_frames(&mut self, custom: bool) {
        self.profile.close();
        if custom {
            self.profile.close();
        }
    }

    pub(crate) fn open_frame(&mut self, name: String, offset: u32) {
        let (line, col) = self.file.map_or((0, 0), |f| f.line_col(offset));
        let name = if name.len() > 200 { truncate_name(&name) } else { name };
        self.profile.open(FrameKey { name, file: self.file_path(), line, col });
    }

    // ---- tags --------------------------------------------------------------------------------

    fn render_tag(&mut self, tag: &'t LiquidTag<'t>, out: &mut String) -> Flow {
        match tag.name {
            "assign" => {
                if let Markup::Node(m) = &tag.markup {
                    if let Node::AssignMarkup(a) = &**m {
                        let v = self.eval(&a.value);
                        self.count("assign", 1.0);
                        self.assign(a.name, v);
                    }
                }
                Flow::Normal
            }
            "capture" => {
                let mut buf = String::new();
                let flow = self.render_children(tag, &mut buf);
                self.count("string_byte", buf.len() as f64);
                if let Markup::Node(m) = &tag.markup {
                    if let Node::VariableLookup(v) = &**m {
                        if let Some(name) = v.name {
                            self.count("assign", 1.0);
                            self.assign(name, Value::str(buf));
                        }
                    }
                }
                flow
            }
            "echo" => {
                if let Markup::Node(m) = &tag.markup {
                    let start = out.len();
                    let v = self.eval(m);
                    v.render_into(out);
                    self.count("output_byte", (out.len() - start) as f64);
                }
                Flow::Normal
            }
            "increment" | "decrement" => {
                if let Markup::Node(m) = &tag.markup {
                    if let Node::VariableLookup(v) = &**m {
                        let name: Rc<str> = Rc::from(v.name.unwrap_or(""));
                        let c = self.counters.entry(name).or_insert(0);
                        if tag.name == "increment" {
                            out.push_str(&c.to_string());
                            *c += 1;
                        } else {
                            *c -= 1;
                            out.push_str(&c.to_string());
                        }
                    }
                }
                Flow::Normal
            }
            "cycle" => {
                if let Markup::Node(m) = &tag.markup {
                    if let Node::CycleMarkup(c) = &**m {
                        let key = match &c.group_name {
                            Some(g) => self.eval(g).render(),
                            None => self.src(tag.markup_position).to_string(),
                        };
                        let i = self.cycles.entry(key).or_insert(0);
                        let idx = *i % c.args.len().max(1);
                        *i += 1;
                        if let Some(arg) = c.args.get(idx) {
                            let v = self.eval(arg);
                            v.render_into(out);
                        }
                    }
                }
                Flow::Normal
            }
            "if" | "unless" => self.render_if(tag, out),
            "case" => self.render_case(tag, out),
            "for" => self.render_for(tag, out),
            "tablerow" => self.render_tablerow(tag, out),
            "break" => Flow::Break,
            "continue" => Flow::Continue,
            "render" | "include" => {
                if let Markup::Node(m) = &tag.markup {
                    if let Node::RenderMarkup(r) = &**m {
                        self.render_partial(tag, r, out);
                    }
                }
                Flow::Normal
            }
            "section" => {
                if let Markup::Node(m) = &tag.markup {
                    if let Node::SectionMarkup(s) = &**m {
                        let name = self.eval(&s.name).render();
                        crate::page::render_static_section(self, &name, out);
                    }
                }
                Flow::Normal
            }
            "sections" => {
                let name = match &tag.markup {
                    Markup::Node(m) => self.eval(m).render(),
                    Markup::Str(s) => s.trim().trim_matches(['"', '\'']).to_string(),
                    _ => String::new(),
                };
                crate::page::render_section_group(self, &name, out);
                Flow::Normal
            }
            "content_for" => {
                if let Markup::Node(m) = &tag.markup {
                    if let Node::ContentForMarkup(c) = &**m {
                        let kind = self.eval(&c.content_for_type).render();
                        let mut args = Vec::new();
                        for a in &c.args {
                            if let Node::NamedArgument(n) = a {
                                let v = self.eval(&n.value);
                                args.push((n.name.to_string(), v));
                            }
                        }
                        crate::page::render_content_for(self, &kind, &args, tag.position.start, out);
                    }
                }
                Flow::Normal
            }
            "form" => self.render_form(tag, out),
            "paginate" => self.render_paginate(tag, out),
            "layout" => {
                let name = match &tag.markup {
                    Markup::Node(m) => match &**m {
                        Node::VariableLookup(v) if v.name == Some("none") => "none".to_string(),
                        other => self.eval(other).render(),
                    },
                    Markup::Str(s) => s.trim().trim_matches(['"', '\'']).to_string(),
                    _ => String::new(),
                };
                self.layout = Some(if name.is_empty() { "none".into() } else { name });
                Flow::Normal
            }
            "ifchanged" => {
                let mut buf = String::new();
                let flow = self.render_children(tag, &mut buf);
                let id = tag.id.0;
                if self.ifchanged.get(&id) != Some(&buf) {
                    out.push_str(&buf);
                    self.ifchanged.insert(id, buf);
                }
                flow
            }
            "#" | "comment" | "doc" => Flow::Normal,
            _ => {
                self.count("tag:unknown", 1.0);
                self.unsupported.insert(format!("tag {}", tag.name));
                // Unknown block tags still render their body, as app tags usually wrap markup.
                if tag.children.is_some() {
                    return self.render_children(tag, out);
                }
                Flow::Normal
            }
        }
    }

    /// Renders a non-branched block tag's children.
    fn render_children(&mut self, tag: &'t LiquidTag<'t>, out: &mut String) -> Flow {
        let Some(children) = &tag.children else { return Flow::Normal };
        let start = tag.block_start_position.end;
        let end = tag.block_end_position.map_or(tag.position.end, |p| p.start);
        let trim_last = tag.delimiter_whitespace_start == Some("-");
        self.render_body(children, Position::new(start, end), tag.whitespace_end == "-", trim_last, out)
    }

    /// Renders one branch of a branched tag (`if`, `for`, `case`, ...).
    fn render_branch(&mut self, tag: &'t LiquidTag<'t>, index: usize, out: &mut String) -> Flow {
        let Some(children) = &tag.children else { return Flow::Normal };
        let Some(Node::LiquidBranch(b)) = children.get(index) else { return Flow::Normal };
        let start = if b.name.is_none() { b.position.start } else { b.block_start_position.end };
        let trim_first = if b.name.is_none() { tag.whitespace_end == "-" } else { b.whitespace_end == "-" };
        let trim_last = match children.get(index + 1) {
            Some(Node::LiquidBranch(next)) => next.whitespace_start == "-",
            _ => tag.delimiter_whitespace_start == Some("-"),
        };
        self.render_body(&b.children, Position::new(start, b.position.end), trim_first, trim_last, out)
    }

    fn branches(tag: &'t LiquidTag<'t>) -> Vec<(usize, &'t lq_syntax::LiquidBranch<'t>)> {
        tag.children
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .enumerate()
            .filter_map(|(i, n)| match n {
                Node::LiquidBranch(b) => Some((i, &**b)),
                _ => None,
            })
            .collect()
    }

    fn render_if(&mut self, tag: &'t LiquidTag<'t>, out: &mut String) -> Flow {
        for (i, b) in Self::branches(tag) {
            let take = match b.name {
                None => {
                    let c = self.eval_markup(&tag.markup);
                    let t = c.is_truthy();
                    if tag.name == "unless" { !t } else { t }
                }
                Some("elsif") => self.eval_markup(&b.markup).is_truthy(),
                Some("else") => true,
                Some(_) => false,
            };
            if take {
                return self.render_branch(tag, i, out);
            }
        }
        Flow::Normal
    }

    fn render_case(&mut self, tag: &'t LiquidTag<'t>, out: &mut String) -> Flow {
        let subject = self.eval_markup(&tag.markup);
        let mut matched = false;
        for (i, b) in Self::branches(tag) {
            match b.name {
                Some("when") => {
                    let values: Vec<&'t Node<'t>> = match &b.markup {
                        Markup::Node(n) => vec![&**n],
                        Markup::List(l) => l.iter().collect(),
                        _ => vec![],
                    };
                    let hit = values.into_iter().any(|v| {
                        let v = self.eval(v);
                        self.count("compare", 1.0);
                        v.liquid_eq(&subject)
                    });
                    if hit {
                        matched = true;
                        let flow = self.render_branch(tag, i, out);
                        if flow != Flow::Normal {
                            return flow;
                        }
                    }
                }
                Some("else") if !matched => return self.render_branch(tag, i, out),
                _ => {}
            }
        }
        Flow::Normal
    }

    fn loop_items(&mut self, tag: &'t LiquidTag<'t>, collection: &'t Node<'t>, args: &'t [Node<'t>], reversed: bool) -> Vec<Value> {
        let coll = self.eval(collection);
        let mut items = coll.to_items();
        let mut offset = 0usize;
        let mut limit = None;
        for a in args {
            if let Node::NamedArgument(n) = a {
                match &*n.name {
                    "offset" => {
                        if let Node::VariableLookup(v) = &n.value {
                            if v.name == Some("continue") {
                                continue;
                            }
                        }
                        offset = self.eval(&n.value).as_int().unwrap_or(0).max(0) as usize;
                    }
                    "limit" => limit = Some(self.eval(&n.value).as_int().unwrap_or(0).max(0) as usize),
                    _ => {}
                }
            }
        }
        items = items.into_iter().skip(offset).collect();
        if let Some(l) = limit {
            items.truncate(l);
        }
        if reversed {
            items.reverse();
        }
        // Calibrated shapes: make this loop run as often as it did on the real store.
        let key = format!("{}:{}", self.file_path(), self.line_of(tag.position.start));
        if let Some(&n) = self.loop_shapes.get(&key) {
            if items.is_empty() {
                items = vec![Value::Nil; n];
            } else {
                let base = items.clone();
                items = (0..n).map(|i| base[i % base.len()].clone()).collect();
            }
        }
        items
    }

    fn render_for(&mut self, tag: &'t LiquidTag<'t>, out: &mut String) -> Flow {
        let Markup::Node(m) = &tag.markup else { return Flow::Normal };
        let Node::ForMarkup(f) = &**m else { return Flow::Normal };
        let items = self.loop_items(tag, &f.collection, &f.args, f.reversed);
        let branches = Self::branches(tag);
        if items.is_empty() {
            if let Some((i, _)) = branches.iter().find(|(_, b)| b.name == Some("else")) {
                return self.render_branch(tag, *i, out);
            }
            return Flow::Normal;
        }
        let parent = self.lookup_var("forloop");
        let len = items.len();
        self.scopes.push(Scope::new());
        for (i, item) in items.into_iter().enumerate() {
            self.count("iter", 1.0);
            let scope = self.scopes.last_mut().unwrap();
            scope.insert(Rc::from(f.variable_name), item);
            scope.insert(Rc::from("forloop"), forloop("forloop", i, len, parent.clone()));
            let flow = self.render_branch(tag, 0, out);
            if flow == Flow::Break || self.stopped {
                break;
            }
        }
        self.scopes.pop();
        Flow::Normal
    }

    fn render_tablerow(&mut self, tag: &'t LiquidTag<'t>, out: &mut String) -> Flow {
        let Markup::Node(m) = &tag.markup else { return Flow::Normal };
        let Node::ForMarkup(f) = &**m else { return Flow::Normal };
        let items = self.loop_items(tag, &f.collection, &f.args, f.reversed);
        let mut cols = 0usize;
        for a in &f.args {
            if let Node::NamedArgument(n) = a {
                if n.name == "cols" {
                    cols = self.eval(&n.value).as_int().unwrap_or(0).max(0) as usize;
                }
            }
        }
        let len = items.len();
        self.scopes.push(Scope::new());
        out.push_str("<tr class=\"row1\">\n");
        let mut row = 1;
        for (i, item) in items.into_iter().enumerate() {
            self.count("iter", 1.0);
            let col = if cols > 0 { i % cols } else { i };
            if cols > 0 && i > 0 && col == 0 {
                row += 1;
                out.push_str(&format!("</tr>\n<tr class=\"row{row}\">"));
            }
            let scope = self.scopes.last_mut().unwrap();
            scope.insert(Rc::from(f.variable_name), item);
            scope.insert(Rc::from("tablerowloop"), forloop("tablerowloop", i, len, Value::Nil));
            out.push_str(&format!("<td class=\"col{}\">", col + 1));
            let flow = self.render_branch(tag, 0, out);
            out.push_str("</td>");
            if flow == Flow::Break || self.stopped {
                break;
            }
        }
        out.push_str("</tr>\n");
        self.scopes.pop();
        Flow::Normal
    }

    fn render_partial(&mut self, tag: &'t LiquidTag<'t>, r: &'t lq_syntax::RenderMarkup<'t>, out: &mut String) {
        let target = self.eval(&r.snippet);
        // `{% render block %}` renders an app or theme block, not a snippet.
        if let Value::Object(o) = &target {
            if &*o.ty == "block" {
                let ty = o.get("type").map(|t| t.render()).unwrap_or_default();
                let block = target.clone();
                crate::page::render_block_value(self, &ty, block, tag.position.start, out);
                return;
            }
        }
        let name = target.render();
        let path = format!("snippets/{name}.liquid");
        if self.theme.get(&path).is_none() {
            self.error(tag.position.start, &format!("Could not find asset {path}"), out);
            return;
        }
        if self.depth >= self.limits.max_depth {
            self.error(tag.position.start, "Nesting too deep", out);
            return;
        }
        let mut args = Scope::new();
        for a in &r.args {
            if let Node::NamedArgument(n) = a {
                let v = self.eval(&n.value);
                args.insert(Rc::from(&*n.name), v);
            }
        }
        let alias: Rc<str> = match &r.alias {
            Some(Node::RenderAliasExpression(a)) => Rc::from(a.value),
            _ => Rc::from(name.as_str()),
        };
        let (items, is_for) = match &r.variable {
            Some(Node::RenderVariableExpression(v)) => {
                let val = self.eval(&v.name);
                match v.kind {
                    lq_syntax::RenderVariableKind::For => (val.to_items(), true),
                    lq_syntax::RenderVariableKind::With => (vec![val], false),
                }
            }
            _ => (vec![], false),
        };
        let isolated = tag.name == "render";
        let saved = if isolated { Some(std::mem::take(&mut self.scopes)) } else { None };
        let saved_paginate = if isolated { Some(std::mem::take(&mut self.paginate)) } else { None };
        self.depth += 1;
        let runs = if is_for { items.len() } else { 1 };
        let len = runs;
        for i in 0..runs {
            self.count(if isolated { "render" } else { "include" }, 1.0);
            let mut scope = args.clone();
            if is_for {
                scope.insert(alias.clone(), items[i].clone());
                scope.insert(Rc::from("forloop"), forloop("forloop", i, len, Value::Nil));
            } else if let Some(v) = items.first() {
                scope.insert(alias.clone(), v.clone());
            }
            if isolated {
                self.scopes = vec![scope];
            } else {
                self.scopes.push(scope);
            }
            self.render_template(&path, false, out);
            if !isolated {
                self.scopes.pop();
            }
            if self.stopped {
                break;
            }
        }
        self.depth -= 1;
        if let Some(s) = saved {
            self.scopes = s;
        }
        if let Some(p) = saved_paginate {
            self.paginate = p;
        }
    }

    pub(crate) fn with_depth<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> Option<R> {
        if self.depth >= self.limits.max_depth {
            return None;
        }
        self.depth += 1;
        let r = f(self);
        self.depth -= 1;
        Some(r)
    }

    fn render_form(&mut self, tag: &'t LiquidTag<'t>, out: &mut String) -> Flow {
        self.count("tag:form", 1.0);
        let args: Vec<&'t Node<'t>> = match &tag.markup {
            Markup::List(l) => l.iter().collect(),
            Markup::Node(n) => vec![&**n],
            _ => vec![],
        };
        let kind = args.first().map(|a| self.eval(a).render()).unwrap_or_default();
        let mut attrs = String::new();
        let mut id = format!("{kind}-form");
        for a in args.iter().skip(1) {
            match a {
                Node::NamedArgument(n) => {
                    let v = self.eval(&n.value).render();
                    if n.name == "id" {
                        id = v.clone();
                    }
                    attrs.push_str(&format!(" {}=\"{}\"", n.name, v));
                }
                other => {
                    let _ = self.eval(other);
                }
            }
        }
        let action = match kind.as_str() {
            "product" => "/cart/add",
            "cart" => "/cart",
            "customer_login" => "/account/login",
            "create_customer" => "/account",
            "contact" | "customer" => "/contact#contact_form",
            "localization" => "/localization",
            "new_comment" => "/blogs/news/comments",
            _ => "/",
        };
        out.push_str(&format!(
            "<form method=\"post\" action=\"{action}\" id=\"{id}\" accept-charset=\"UTF-8\"{attrs}>\
             <input type=\"hidden\" name=\"form_type\" value=\"{kind}\" /><input type=\"hidden\" name=\"utf8\" value=\"✓\" />"
        ));
        let mut form = Object::new("form");
        form.set("id", Value::str(&id));
        form.set("errors", Value::Nil);
        form.set("posted_successfully?", Value::Bool(false));
        form.set("email", Value::Nil);
        let mut scope = Scope::new();
        scope.insert(Rc::from("form"), Value::object(form));
        self.scopes.push(scope);
        let flow = self.render_children(tag, out);
        self.scopes.pop();
        out.push_str("</form>");
        flow
    }

    fn render_paginate(&mut self, tag: &'t LiquidTag<'t>, out: &mut String) -> Flow {
        self.count("tag:paginate", 1.0);
        let Markup::Node(m) = &tag.markup else { return self.render_children(tag, out) };
        let Node::PaginateMarkup(p) = &**m else { return self.render_children(tag, out) };
        let coll = self.eval(&p.collection);
        let size = self.eval(&p.page_size).as_int().unwrap_or(50).clamp(1, 250) as usize;
        let all = coll.to_items();
        let total = match &coll {
            Value::Object(o) => o.get("__count").and_then(Value::as_int).map_or(all.len(), |c| c as usize),
            _ => all.len(),
        };
        let page: Vec<Value> = all.into_iter().take(size).collect();
        let pages = total.div_ceil(size).max(1);
        let mut po = Object::new("paginate");
        po.set("current_page", Value::Int(1));
        po.set("current_offset", Value::Int(0));
        po.set("items", Value::Int(total as i64));
        po.set("page_size", Value::Int(size as i64));
        po.set("pages", Value::Int(pages as i64));
        po.set("page_param", Value::str("page"));
        po.set("previous", Value::Nil);
        let mut parts = Vec::new();
        for n in 1..=pages.min(5) {
            let mut part = Object::new("part");
            part.set("title", Value::Int(n as i64));
            part.set("url", Value::str(format!("?page={n}")));
            part.set("is_link", Value::Bool(n != 1));
            parts.push(Value::object(part));
        }
        po.set("parts", Value::array(parts));
        if pages > 1 {
            let mut next = Object::new("part");
            next.set("title", Value::str("Next »"));
            next.set("url", Value::str("?page=2"));
            next.set("is_link", Value::Bool(true));
            po.set("next", Value::object(next));
        } else {
            po.set("next", Value::Nil);
        }
        let key = self.src(p.collection.position()).to_string();
        self.paginate.push((key, Value::array(page)));
        let mut scope = Scope::new();
        scope.insert(Rc::from("paginate"), Value::object(po));
        self.scopes.push(scope);
        let flow = self.render_children(tag, out);
        self.scopes.pop();
        self.paginate.pop();
        flow
    }

    // ---- variables ---------------------------------------------------------------------------

    pub(crate) fn assign(&mut self, name: &str, v: Value) {
        if let Some(scope) = self.scopes.first_mut() {
            scope.insert(Rc::from(name), v);
        }
    }

    pub(crate) fn lookup_var(&mut self, name: &str) -> Value {
        // Objects Shopify builds per request when first read.
        if matches!(name, "content_for_header" | "content_for_layout") && self.scopes.iter().all(|s| !s.contains_key(name)) {
            self.count(&format!("global:{name}"), 1.0);
        }
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.get(name) {
                return v.clone();
            }
        }
        self.globals.get(name).cloned().unwrap_or(Value::Nil)
    }

    /// Reads `key` on `v`, counting the lookup and any database-backed access.
    pub(crate) fn get_prop(&mut self, v: &Value, key: &Value) -> Value {
        self.count("lookup", 1.0);
        match v {
            Value::Object(o) => {
                let k = key.render();
                if let Some(op) = self.model.db_op(&o.ty, &k) {
                    let seen_key = (o.id, Rc::from(k.as_str()));
                    if self.db_seen.insert(seen_key) {
                        self.count(&op, 1.0);
                    }
                }
                if let Some(x) = o.get(&k) {
                    return x.clone();
                }
                match k.as_str() {
                    "size" => Value::Int(match o.get("__items") {
                        Some(Value::Array(a)) => a.len() as i64,
                        _ => o.fields.len() as i64,
                    }),
                    "first" | "last" => match o.get("__items") {
                        Some(Value::Array(a)) => {
                            (if k == "first" { a.first() } else { a.last() }).cloned().unwrap_or_default()
                        }
                        _ => Value::Nil,
                    },
                    _ => match key {
                        Value::Int(i) => match o.get("__items") {
                            Some(Value::Array(a)) => index(a, *i),
                            _ => Value::Nil,
                        },
                        _ => Value::Nil,
                    },
                }
            }
            Value::Array(a) => match key {
                Value::Int(i) => index(a, *i),
                Value::Float(f) => index(a, *f as i64),
                Value::Str(s) => match &**s {
                    "size" => Value::Int(a.len() as i64),
                    "first" => a.first().cloned().unwrap_or_default(),
                    "last" => a.last().cloned().unwrap_or_default(),
                    _ => Value::Nil,
                },
                _ => Value::Nil,
            },
            Value::Str(s) => match key {
                Value::Str(k) if &**k == "size" => Value::Int(s.chars().count() as i64),
                Value::Str(k) if &**k == "first" => s.chars().next().map(|c| Value::str(c.to_string())).unwrap_or_default(),
                Value::Str(k) if &**k == "last" => s.chars().last().map(|c| Value::str(c.to_string())).unwrap_or_default(),
                _ => Value::Nil,
            },
            Value::Range(a, b) => match key {
                Value::Str(k) if &**k == "size" => Value::Int((b - a + 1).max(0)),
                Value::Str(k) if &**k == "first" => Value::Int(*a),
                Value::Str(k) if &**k == "last" => Value::Int(*b),
                _ => Value::Nil,
            },
            _ => Value::Nil,
        }
    }

    fn eval_markup(&mut self, m: &'t Markup<'t>) -> Value {
        match m {
            Markup::Node(n) => self.eval(n),
            _ => Value::Nil,
        }
    }

    pub(crate) fn eval(&mut self, node: &'t Node<'t>) -> Value {
        match node {
            Node::String(s) => Value::str(s.value),
            Node::Number(n) => {
                if let Ok(i) = n.value.parse::<i64>() {
                    Value::Int(i)
                } else {
                    Value::Float(n.value.parse().unwrap_or(0.0))
                }
            }
            Node::LiquidLiteral(l) => match l.keyword {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                "empty" => Value::Empty,
                "blank" => Value::Blank,
                _ => Value::Nil,
            },
            Node::Range(r) => {
                let a = self.eval(&r.start).as_int().unwrap_or(0);
                let b = self.eval(&r.end).as_int().unwrap_or(0);
                Value::Range(a, b)
            }
            Node::VariableLookup(v) => {
                if !self.paginate.is_empty() {
                    let text = self.src(v.position);
                    if let Some((_, page)) = self.paginate.iter().rev().find(|(k, _)| k == text) {
                        return page.clone();
                    }
                }
                let mut lookups = v.lookups.iter();
                let mut value = match v.name {
                    Some(name) => {
                        self.count("lookup", 1.0);
                        self.lookup_var(name)
                    }
                    None => match lookups.next() {
                        Some(first) => {
                            let key = self.eval(first).render();
                            self.count("lookup", 1.0);
                            self.lookup_var(&key)
                        }
                        None => Value::Nil,
                    },
                };
                for l in lookups {
                    let key = match l {
                        Node::String(s) => Value::str(s.value),
                        other => self.eval(other),
                    };
                    value = self.get_prop(&value, &key);
                }
                value
            }
            Node::Comparison(c) => {
                let l = self.eval(&c.left);
                let r = self.eval(&c.right);
                self.count("compare", 1.0);
                Value::Bool(compare(c.comparator, &l, &r))
            }
            Node::LogicalExpression(e) => {
                let l = self.eval(&e.left).is_truthy();
                let v = match e.relation {
                    lq_syntax::LogicalRelation::And => l && self.eval(&e.right).is_truthy(),
                    lq_syntax::LogicalRelation::Or => l || self.eval(&e.right).is_truthy(),
                };
                Value::Bool(v)
            }
            Node::BooleanExpression(b) => Value::Bool(self.eval(&b.condition).is_truthy()),
            Node::LiquidVariable(v) => {
                let mut value = self.eval(&v.expression);
                for f in &v.filters {
                    if let Node::LiquidFilter(f) = f {
                        let mut args = Vec::new();
                        let mut kwargs = Vec::new();
                        for a in &f.args {
                            match a {
                                Node::NamedArgument(n) => {
                                    let x = self.eval(&n.value);
                                    kwargs.push((n.name.to_string(), x));
                                }
                                other => args.push(self.eval(other)),
                            }
                        }
                        let op = format!("filter:{}", f.name);
                        self.open_plain(&op, "");
                        self.count(&op, 1.0);
                        value = crate::filters::apply(self, f.name, value, args, kwargs);
                        self.profile.close();
                    }
                }
                value
            }
            _ => Value::Nil,
        }
    }
}

fn index(a: &[Value], i: i64) -> Value {
    let i = if i < 0 { a.len() as i64 + i } else { i };
    if i < 0 {
        return Value::Nil;
    }
    a.get(i as usize).cloned().unwrap_or_default()
}

pub(crate) fn compare(op: &str, l: &Value, r: &Value) -> bool {
    use std::cmp::Ordering::*;
    match op {
        "==" => l.liquid_eq(r),
        "!=" | "<>" => !l.liquid_eq(r),
        "contains" => l.contains(r),
        "<" => l.compare(r) == Some(Less),
        ">" => l.compare(r) == Some(Greater),
        "<=" => matches!(l.compare(r), Some(Less | Equal)),
        ">=" => matches!(l.compare(r), Some(Greater | Equal)),
        _ => false,
    }
}

fn forloop(ty: &str, i: usize, len: usize, parent: Value) -> Value {
    let mut o = Object::new(ty);
    o.set("index", Value::Int(i as i64 + 1));
    o.set("index0", Value::Int(i as i64));
    o.set("rindex", Value::Int((len - i) as i64));
    o.set("rindex0", Value::Int((len - i - 1) as i64));
    o.set("first", Value::Bool(i == 0));
    o.set("last", Value::Bool(i + 1 == len));
    o.set("length", Value::Int(len as i64));
    if ty == "tablerowloop" {
        o.set("col", Value::Int(i as i64 + 1));
        o.set("col0", Value::Int(i as i64));
        o.set("row", Value::Int(1));
    } else {
        o.set("parentloop", parent);
    }
    Value::object(o)
}

/// Whether a node trims the whitespace before it (`{%-` or `{{-`).
fn whitespace_start(node: &Node<'_>) -> bool {
    match node {
        Node::LiquidTag(t) => t.whitespace_start == "-",
        Node::LiquidRawTag(t) => t.whitespace_start == "-",
        Node::LiquidVariableOutput(o) => o.whitespace_start == "-",
        _ => false,
    }
}

/// Whether a node trims the whitespace after it: the closing tag's `-%}` for blocks.
fn whitespace_after(node: &Node<'_>) -> bool {
    match node {
        Node::LiquidTag(t) => match (&t.children, t.delimiter_whitespace_end) {
            (Some(_), Some(end)) => end == "-",
            _ => t.whitespace_end == "-",
        },
        Node::LiquidRawTag(t) => t.delimiter_whitespace_end == "-",
        Node::LiquidVariableOutput(o) => o.whitespace_end == "-",
        _ => false,
    }
}

fn truncate_name(name: &str) -> String {
    let mut end = 200;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &name[..end])
}
