//! Replays the golden corpus recorded from upstream's specs (`tests/goldens`): the
//! default mock dependencies of `test-helper.ts`, MockFileSystem semantics, rootUri
//! `file:/`, plus recorded mock overrides.

use crate::api::{CheckDef, CheckImpl, CheckMeta, Divergences, Severity, SourceCodeType};
use crate::context_utils::{MetafieldDefinition, MetafieldDefinitionMap};
use crate::deps::Dependencies;
use crate::fixes::{apply_fix_to_string, fix_all, fix_edits, Fix};
use crate::fs::{FileSystem, MockFileSystem};
use crate::graph::{Location, Reference, ReferenceType};
use crate::liquid_doc::{extract_doc_definition, DocContent, DocDefinition, LiquidDocParameter};
use crate::registry::REGISTRY;
use crate::run::{check_with_registry, Offense, Position};
use crate::source::{to_source_code, Ast, SourceCode, Theme};
use crate::to_schema::{self, fixed_schema, to_schema, IsValidSchema, StaticBlockDef, ThemeSchema, ThemeSchemaType};
use crate::utils::styles::{css_classes_from_asset_uri, css_classes_from_liquid_uri};
use lq_json::validator::{JsError, JsonValidator, SchemaDefinition};
use lq_json::{uri, Object, Value};
use lq_theme::config::{Config, Mode};
use lq_theme::docs::{Access, FilterEntry, ObjectEntry, Parameter, Parent, ReturnType, TagEntry, ThemeDocset};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

const ROOT_URI: &str = "file:///";

/// `disabled-checks/test-checks.ts`.
mod test_checks {
    use crate::api::*;
    use lq_syntax::Node;

    pub const LIQUID_FILTER: CheckImpl = CheckImpl::Liquid {
        create: |_| Some(Box::new(LiquidFilter)),
        enter: NodeMask::of(&[NodeKind::LiquidFilter]),
        exit: NodeMask::NONE,
    };

    struct LiquidFilter;

    impl<'a> LiquidCheck<'a> for LiquidFilter {
        fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
            let p = node.position();
            cx.report(LiquidProblem::new("Liquid filter can not be used", p.start, p.end));
        }
    }

    pub const RENDER_MARKUP: CheckImpl = CheckImpl::Liquid {
        create: |_| Some(Box::new(RenderMarkup)),
        enter: NodeMask::of(&[NodeKind::RenderMarkup]),
        exit: NodeMask::NONE,
    };

    struct RenderMarkup;

    impl<'a> LiquidCheck<'a> for RenderMarkup {
        fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
            let Node::RenderMarkup(m) = node else { return };
            let Node::String(s) = &m.snippet else { return };
            let p = m.snippet.position();
            cx.report(LiquidProblem::new(format!("'{}.liquid' can not be rendered", s.value), p.start, p.end));
        }
    }
}

const fn test_def(code: &'static str, imp: CheckImpl) -> CheckDef {
    CheckDef {
        meta: CheckMeta {
            code,
            name: code,
            description: "",
            recommended: true,
            url: None,
            severity: Severity::Error,
            kind: SourceCodeType::LiquidHtml,
            aliases: &[],
            targets: &[],
            schema: &[],
            deprecated: false,
        },
        imp,
    }
}

static TEST_REGISTRY: &[CheckDef] =
    &[test_def("LiquidFilter", test_checks::LIQUID_FILTER), test_def("RenderMarkup", test_checks::RENDER_MARKUP)];

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn leak_slice<T>(v: Vec<T>) -> &'static [T] {
    Box::leak(v.into_boxed_slice())
}

fn s_field(v: &Value, k: &str) -> &'static str {
    leak(v.get(k).and_then(Value::as_str).unwrap_or(""))
}

fn opt_field(v: &Value, k: &str) -> Option<&'static str> {
    v.get(k).and_then(Value::as_str).map(leak)
}

fn truthy(v: &Value, k: &str) -> bool {
    v.get(k).is_some_and(Value::is_truthy)
}

fn parameters(v: &Value) -> &'static [Parameter] {
    leak_slice(
        v.get("parameters")
            .and_then(Value::as_array)
            .unwrap_or(&[])
            .iter()
            .map(|p| Parameter {
                name: s_field(p, "name"),
                positional: truthy(p, "positional"),
                required: truthy(p, "required"),
                types: leak_slice(
                    p.get("types")
                        .and_then(Value::as_array)
                        .unwrap_or(&[])
                        .iter()
                        .filter_map(Value::as_str)
                        .map(leak)
                        .collect(),
                ),
            })
            .collect(),
    )
}

fn return_types(v: &Value) -> &'static [ReturnType] {
    leak_slice(
        v.get("return_type")
            .and_then(Value::as_array)
            .unwrap_or(&[])
            .iter()
            .map(|r| ReturnType { ty: s_field(r, "type"), name: s_field(r, "name"), array_value: s_field(r, "array_value") })
            .collect(),
    )
}

fn to_filter(v: &Value) -> FilterEntry {
    FilterEntry {
        name: s_field(v, "name"),
        deprecated: truthy(v, "deprecated"),
        deprecation_reason: opt_field(v, "deprecation_reason"),
        parameters: parameters(v),
        return_type: return_types(v),
    }
}

fn to_object(v: &Value) -> ObjectEntry {
    ObjectEntry {
        name: s_field(v, "name"),
        deprecated: truthy(v, "deprecated"),
        deprecation_reason: opt_field(v, "deprecation_reason"),
        access: v.get("access").filter(|a| a.is_truthy()).map(|a| Access {
            global: truthy(a, "global"),
            parents: leak_slice(
                a.get("parents")
                    .and_then(Value::as_array)
                    .unwrap_or(&[])
                    .iter()
                    .map(|p| Parent { object: s_field(p, "object"), property: s_field(p, "property") })
                    .collect(),
            ),
            template: leak_slice(
                a.get("template")
                    .and_then(Value::as_array)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Value::as_str)
                    .map(leak)
                    .collect(),
            ),
        }),
        return_type: return_types(v),
    }
}

fn to_tag(v: &Value) -> TagEntry {
    TagEntry {
        name: s_field(v, "name"),
        deprecated: truthy(v, "deprecated"),
        deprecation_reason: opt_field(v, "deprecation_reason"),
        parameters: parameters(v),
    }
}

/// A docset from data, the helper's default or a recorded one.
struct MockDocset {
    augmented: bool,
    filters: Vec<FilterEntry>,
    objects: Vec<ObjectEntry>,
    liquid_drops: Vec<ObjectEntry>,
    tags: Vec<TagEntry>,
    translations: HashSet<String>,
}

impl ThemeDocset for MockDocset {
    fn is_augmented(&self) -> bool {
        self.augmented
    }
    fn filters(&self) -> &[FilterEntry] {
        &self.filters
    }
    fn objects(&self) -> &[ObjectEntry] {
        &self.objects
    }
    fn liquid_drops(&self) -> &[ObjectEntry] {
        &self.liquid_drops
    }
    fn tags(&self) -> &[TagEntry] {
        &self.tags
    }
    fn has_system_translation(&self, key: &str) -> bool {
        self.translations.contains(key)
    }
    fn system_translation_count(&self) -> usize {
        self.translations.len()
    }
}

fn entries_of<T>(v: &Value, key: &str, f: impl Fn(&Value) -> T) -> Vec<T> {
    v.get(key).and_then(Value::as_array).unwrap_or(&[]).iter().map(f).collect()
}

fn docset_from(v: &Value) -> MockDocset {
    MockDocset {
        augmented: truthy(v, "isAugmented"),
        filters: entries_of(v, "filters", to_filter),
        objects: entries_of(v, "objects", to_object),
        liquid_drops: entries_of(v, "liquidDrops", to_object),
        tags: entries_of(v, "tags", to_tag),
        translations: v
            .get("systemTranslations")
            .and_then(Value::as_object)
            .map(|o| o.keys().map(String::from).collect())
            .unwrap_or_default(),
    }
}

/// The `themeDocset` of `test-helper.ts`.
fn default_docset() -> MockDocset {
    const DEFAULT: &str = r#"{
      "filters": [
        {"name":"item_count_for_variant"},{"name":"link_to_type"},{"name":"link_to_vendor"},
        {"name":"append"},{"name":"upcase"},{"name":"downcase"},{"name":"color_to_rgb"},
        {"name":"hex_to_rgba","deprecated":true,"deprecation_reason":"`hex_to_rgba` has been replaced by [`color_to_rgb`](/do..."},
        {"name":"currency_selector","deprecated":true,"deprecation_reason":"Deprecated without a direct replacement because the [cur..."},
        {"name":"article_img_url","deprecated":true,"deprecation_reason":"`article_img_url` has been replaced by [`image_url`](/d..."},
        {"name":"collection_img_url","deprecated":true,"deprecation_reason":"`collection_img_url` has been replaced by [`image_url`](..."},
        {"name":"img_tag","deprecated":true,"deprecation_reason":"`img_tag` has been replaced by [`image_tag`](/docs/api/..."},
        {"name":"img_url","deprecated":true,"deprecation_reason":"`img_url` has been replaced by [`image_url`](/docs/api/..."},
        {"name":"product_img_url","deprecated":true,"deprecation_reason":"`product_img_url` has been replaced by [`image_url`](/d..."}
      ],
      "objects": [
        {"name":"collections"},
        {"name":"product","access":{"global":false,"parents":[],"template":["product"]}},
        {"name":"image","access":{"global":false,"parents":[],"template":[]}},
        {"name":"section","access":{"global":false,"parents":[],"template":[]}},
        {"name":"block","access":{"global":false,"parents":[],"template":[]}},
        {"name":"app","access":{"global":false,"parents":[],"template":[]}},
        {"name":"predictive_search","access":{"global":false,"parents":[],"template":[]}},
        {"name":"recommendations","access":{"global":false,"parents":[],"template":[]}},
        {"name":"comment","access":{"global":false,"parents":[],"template":[]}}
      ],
      "tags": [],
      "systemTranslations": {"shopify.sentence.words_connector": ", "}
    }"#;
    let v = lq_json::parse_json(DEFAULT).expect("default docset");
    let mut d = docset_from(&v);
    d.liquid_drops = d.objects.clone();
    d
}

/// A recorded function dependency: `None` means the default mock.
enum Provider<'v> {
    Default,
    Absent,
    Table(&'v Object),
}

fn provider<'v>(deps: &'v Value, key: &str) -> Provider<'v> {
    match deps.get(key) {
        None => Provider::Default,
        Some(Value::Null) => Provider::Absent,
        Some(Value::Object(o)) => Provider::Table(o),
        Some(_) => Provider::Absent,
    }
}

fn js_error(v: &Value) -> Option<JsError> {
    v.get("$error").and_then(Value::as_str).map(to_schema::error)
}

fn schema_from_record(v: &Value, ty: ThemeSchemaType) -> Option<ThemeSchema<'static>> {
    if js_error(v).is_some() || !matches!(v, Value::Object(_)) {
        return None;
    }
    let result = |k: &str| match v.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(x) => match js_error(x) {
            Some(e) => Err(e),
            None => Ok(Some(x.clone())),
        },
    };
    let ty = match v.get("type").and_then(Value::as_str) {
        Some("block") => ThemeSchemaType::Block,
        Some("section") => ThemeSchemaType::Section,
        Some("app-block") => ThemeSchemaType::AppBlock,
        _ => ty,
    };
    let valid = v.get("validSchema").map(|_| result("validSchema"));
    Some(fixed_schema(
        ty,
        v.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
        v.get("offset").and_then(Value::as_f64).unwrap_or(0.0) as u32,
        leak(v.get("value").and_then(Value::as_str).unwrap_or("")),
        result("parsed"),
        Some(valid.unwrap_or(Ok(None))),
        v.get("staticBlockDefs")
            .and_then(Value::as_array)
            .unwrap_or(&[])
            .iter()
            .map(|d| StaticBlockDef { ty: s_field(d, "type").to_string(), id: s_field(d, "id").to_string() })
            .collect(),
    ))
}

/// Dependencies for one record.
pub struct MockDependencies<'a> {
    theme: &'a Theme<'a>,
    mode: Mode,
    recorded: &'a Value,
    fs: MockFileSystem,
    docset: Option<MockDocset>,
    validator: Option<JsonValidator>,
    schema_cache: Mutex<HashMap<(u8, String), Option<&'a ThemeSchema<'a>>>>,
    doc_cache: Mutex<HashMap<String, Option<&'a DocDefinition>>>,
}

impl<'a> MockDependencies<'a> {
    fn new(theme: &'a Theme<'a>, mode: Mode, record_theme: &Object, recorded: &'a Value) -> Self {
        let fs = match recorded.get("fs").filter(|f| f.is_truthy()) {
            Some(f) => MockFileSystem::new(
                f.get("theme").and_then(Value::as_object).cloned().unwrap_or_default(),
                f.get("rootUri").and_then(Value::as_str).unwrap_or(ROOT_URI),
            ),
            None => {
                let mut with_config = Object::new();
                with_config.insert(".theme-check.yml", Value::String(String::new()));
                for (k, v) in record_theme.iter() {
                    with_config.insert(k, v.clone());
                }
                MockFileSystem::new(with_config, ROOT_URI)
            }
        };
        let docset = match recorded.get("themeDocset") {
            None => Some(default_docset()),
            Some(Value::Null) => None,
            Some(v) => Some(docset_from(v)),
        };
        let validator = recorded.get("jsonValidationSet").filter(|j| j.is_truthy()).map(|j| {
            let list = j.get("schemas").and_then(|s| s.get(mode.as_str())).and_then(Value::as_array).unwrap_or(&[]);
            JsonValidator::new(
                list.iter()
                    .map(|s| SchemaDefinition {
                        uri: s_field(s, "uri").to_string(),
                        file_match: s.get("fileMatch").and_then(Value::as_array).map(|m| {
                            m.iter().filter_map(Value::as_str).map(String::from).collect()
                        }),
                        schema: s_field(s, "schema").to_string(),
                    })
                    .collect(),
            )
        });
        MockDependencies {
            theme,
            mode,
            recorded,
            fs,
            docset,
            validator,
            schema_cache: Mutex::new(HashMap::new()),
            doc_cache: Mutex::new(HashMap::new()),
        }
    }

    fn schema_lookup(&'a self, key: &str, tag: u8, ty: ThemeSchemaType, is_target: fn(&str) -> bool, name: &str) -> Option<&'a ThemeSchema<'a>> {
        if let Some(found) = self.schema_cache.lock().unwrap().get(&(tag, name.to_string())) {
            return *found;
        }
        let computed: Option<&'a ThemeSchema<'a>> = match provider(self.recorded, key) {
            Provider::Absent => None,
            Provider::Table(t) => t
                .get(name)
                .and_then(|v| schema_from_record(v, ty))
                .map(|s| &*Box::leak(Box::new(s))),
            Provider::Default => {
                // `new Map(...)`: the last file with that basename wins.
                let file = self
                    .theme
                    .iter()
                    .rfind(|f| is_target(&f.uri) && uri::basename(&f.uri, Some(".liquid")) == name);
                file.and_then(|f| match &f.ast {
                    Ast::Liquid(_) => to_schema(self.mode, &f.uri, f.liquid_ast(), IsValidSchema::AlwaysValid),
                    Ast::Json(_) => None,
                })
                .map(|s| &*Box::leak(Box::new(s)))
            }
        };
        self.schema_cache.lock().unwrap().insert((tag, name.to_string()), computed);
        computed
    }

    fn refs(&self, key: &str, u: &str) -> Vec<Reference> {
        let Provider::Table(t) = provider(self.recorded, key) else { return Vec::new() };
        t.get(u)
            .and_then(Value::as_array)
            .unwrap_or(&[])
            .iter()
            .map(|r| {
                let loc = |k: &str| Location {
                    uri: r.get(k).and_then(|l| l.get("uri")).and_then(Value::as_str).unwrap_or("").to_string(),
                    range: None,
                };
                Reference {
                    source: loc("source"),
                    target: loc("target"),
                    ty: match r.get("type").and_then(Value::as_str) {
                        Some("indirect") => ReferenceType::Indirect,
                        Some("preset") => ReferenceType::Preset,
                        _ => ReferenceType::Direct,
                    },
                }
            })
            .collect()
    }
}

fn doc_from_record(uri_fallback: &str, v: &Value) -> Option<DocDefinition> {
    if !matches!(v, Value::Object(_)) {
        return None;
    }
    let liquid_doc = v.get("liquidDoc").filter(|d| d.is_truthy()).map(|d| DocContent {
        parameters: d.get("parameters").and_then(Value::as_array).map(|ps| {
            ps.iter()
                .map(|p| LiquidDocParameter {
                    name: s_field(p, "name").to_string(),
                    description: p.get("description").and_then(Value::as_str).map(String::from),
                    ty: p.get("type").and_then(Value::as_str).map(String::from),
                    required: truthy(p, "required"),
                })
                .collect()
        }),
        examples: None,
        description: None,
    });
    Some(DocDefinition {
        uri: v.get("uri").and_then(Value::as_str).unwrap_or(uri_fallback).to_string(),
        liquid_doc,
    })
}

impl<'a> Dependencies<'a> for MockDependencies<'a> {
    fn fs(&'a self) -> &'a dyn FileSystem {
        &self.fs
    }
    fn theme_docset(&'a self) -> Option<&'a dyn ThemeDocset> {
        self.docset.as_ref().map(|d| d as &dyn ThemeDocset)
    }
    fn has_json_validator(&self) -> bool {
        self.validator.is_some()
    }
    fn json_validator(&'a self) -> Option<&'a JsonValidator> {
        self.validator.as_ref()
    }
    fn block_schema(&'a self, name: &str) -> Option<&'a ThemeSchema<'a>> {
        self.schema_lookup("getBlockSchema", 0, ThemeSchemaType::Block, to_schema::is_block, name)
    }
    fn section_schema(&'a self, name: &str) -> Option<&'a ThemeSchema<'a>> {
        self.schema_lookup("getSectionSchema", 1, ThemeSchemaType::Section, to_schema::is_section, name)
    }
    fn app_block_schema(&'a self, name: &str) -> Option<&'a ThemeSchema<'a>> {
        match provider(self.recorded, "getAppBlockSchema") {
            Provider::Table(_) => self.schema_lookup("getAppBlockSchema", 2, ThemeSchemaType::AppBlock, to_schema::is_block, name),
            // The test helper has no default `getAppBlockSchema`.
            _ => None,
        }
    }
    fn doc_definition(&'a self, relative_path: &str) -> Option<&'a DocDefinition> {
        if let Some(found) = self.doc_cache.lock().unwrap().get(relative_path) {
            return *found;
        }
        let computed: Option<DocDefinition> = match provider(self.recorded, "getDocDefinition") {
            Provider::Absent => None,
            Provider::Table(t) => t.get(relative_path).and_then(|v| doc_from_record(relative_path, v)),
            Provider::Default => self.theme.iter().find(|f| f.uri.ends_with(relative_path)).and_then(|f| match &f.ast {
                Ast::Liquid(Ok(doc)) => Some(extract_doc_definition(&f.uri, &doc.root)),
                _ => None,
            }),
        };
        let leaked: Option<&'a DocDefinition> = computed.map(|d| &*Box::leak(Box::new(d)));
        self.doc_cache.lock().unwrap().insert(relative_path.to_string(), leaked);
        leaked
    }
    fn has_css_classes(&self) -> bool {
        !matches!(provider(self.recorded, "getCSSClassesForURI"), Provider::Absent)
    }
    fn css_classes_for_uri(&'a self, u: &str) -> Arc<BTreeSet<String>> {
        match provider(self.recorded, "getCSSClassesForURI") {
            Provider::Default => Arc::new(if u.ends_with(".css") {
                css_classes_from_asset_uri(u, &self.fs)
            } else {
                css_classes_from_liquid_uri(u, &self.fs)
            }),
            Provider::Table(t) => {
                let set = t.get(u).and_then(|v| v.get("$set")).and_then(Value::as_array).unwrap_or(&[]);
                Arc::new(set.iter().filter_map(Value::as_str).map(String::from).collect())
            }
            Provider::Absent => Arc::new(BTreeSet::new()),
        }
    }
    fn has_references(&self) -> bool {
        matches!(provider(self.recorded, "getReferences"), Provider::Table(_))
    }
    fn references(&'a self, u: &str) -> Vec<Reference> {
        self.refs("getReferences", u)
    }
    fn has_dependencies(&self) -> bool {
        matches!(provider(self.recorded, "getDependencies"), Provider::Table(_))
    }
    fn dependencies(&'a self, u: &str) -> Vec<Reference> {
        self.refs("getDependencies", u)
    }
    fn metafield_definitions(&'a self, root_uri: &str) -> Option<MetafieldDefinitionMap> {
        let Provider::Table(t) = provider(self.recorded, "getMetafieldDefinitions") else { return None };
        let v = t.get(root_uri)?;
        Some(MetafieldDefinitionMap(
            crate::context_utils::FETCHED_METAFIELD_CATEGORIES
                .iter()
                .map(|c| {
                    let defs = v.get(c).and_then(Value::as_array).unwrap_or(&[]);
                    (
                        *c,
                        defs.iter()
                            .map(|d| {
                                let f = |k: &str| d.get(k).cloned().unwrap_or(Value::Null);
                                let ty = d.get("type").cloned().unwrap_or(Value::Null);
                                MetafieldDefinition {
                                    key: f("key"),
                                    name: f("name"),
                                    namespace: f("namespace"),
                                    description: f("description"),
                                    type_category: ty.get("category").cloned().unwrap_or(Value::Null),
                                    type_name: ty.get("name").cloned().unwrap_or(Value::Null),
                                }
                            })
                            .collect(),
                    )
                })
                .collect(),
        ))
    }
}

/// `{"$rle":[part,...]}`: a large source, run-length encoded by the recorder.
fn decode_source(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(_) => {
            let mut out = String::new();
            for part in v.get("$rle").and_then(Value::as_array).unwrap_or(&[]) {
                match part {
                    Value::String(s) => out.push_str(s),
                    Value::Array(a) => {
                        let c = a.first().and_then(Value::as_str).unwrap_or("");
                        let n = a.get(1).and_then(Value::as_f64).unwrap_or(0.0) as usize;
                        // `count` is in UTF-16 units.
                        let units = lq_json::js::utf16_len(c).max(1);
                        out.push_str(&c.repeat(n / units));
                    }
                    _ => {}
                }
            }
            out
        }
        _ => String::new(),
    }
}

fn decode_theme(v: Option<&Value>) -> Object {
    let mut out = Object::new();
    if let Some(o) = v.and_then(Value::as_object) {
        for (k, s) in o.iter() {
            out.insert(k, Value::String(decode_source(s)));
        }
    }
    out
}

fn pos_json(p: &Position) -> String {
    format!("{{index:{},line:{},character:{}}}", p.index, p.line, p.character)
}

fn pos_value(v: Option<&Value>) -> String {
    let f = |k: &str| v.and_then(|p| p.get(k)).and_then(Value::as_f64).unwrap_or(-1.0);
    format!("{{index:{},line:{},character:{}}}", f("index"), f("line"), f("character"))
}

/// What a fix produces on a file, as recorded: output, or the fact that it threw.
fn fix_output(source: &str, fix: &Fix) -> Result<String, String> {
    let edits = fix_edits(source, fix)?;
    apply_fix_to_string(source, &edits)
}

fn normalize_recorded_output(v: &Value) -> Option<Result<String, ()>> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(Ok(s.clone())),
        Value::Object(o) if o.contains_key("error") => Some(Err(())),
        Value::Object(_) => Some(Ok(decode_source(v))),
        _ => None,
    }
}

fn same_output(want: Option<Result<String, ()>>, got: Option<Result<String, String>>) -> bool {
    match (want, got) {
        (None, None) => true,
        (Some(Ok(a)), Some(Ok(b))) => a == b,
        (Some(Err(())), Some(Err(_))) => true,
        _ => false,
    }
}

/// One offense against its record: the first difference, or `None`.
fn diff_offense(want: &Value, got: &Offense, sources: &HashMap<String, String>) -> Option<String> {
    let w_s = |k: &str| want.get(k).and_then(Value::as_str).unwrap_or("");
    if w_s("check") != got.check {
        return Some(format!("check: want {} got {}", w_s("check"), got.check));
    }
    if w_s("message") != got.message {
        return Some(format!("message: want {:?} got {:?}", w_s("message"), got.message));
    }
    if w_s("uri") != got.uri {
        return Some(format!("uri: want {} got {}", w_s("uri"), got.uri));
    }
    if want.get("severity").and_then(Value::as_f64) != Some(f64::from(got.severity as u8)) {
        return Some(format!("severity: want {:?} got {:?}", want.get("severity"), got.severity));
    }
    for (name, w, g) in [("start", want.get("start"), &got.start), ("end", want.get("end"), &got.end)] {
        if pos_value(w) != pos_json(g) {
            return Some(format!("{name}: want {} got {}", pos_value(w), pos_json(g)));
        }
    }
    let source = sources.get(&got.uri).map_or("", String::as_str);
    let want_fix = want.get("fix").and_then(normalize_recorded_output);
    let got_fix = got.fix.as_ref().map(|f| fix_output(source, f));
    if !same_output(want_fix.clone(), got_fix.clone()) {
        return Some(format!("fix: want {want_fix:?} got {got_fix:?}"));
    }
    let want_suggest = want.get("suggest").and_then(Value::as_array);
    match (want_suggest, &got.suggest) {
        (None, None) => {}
        (Some(w), Some(g)) => {
            if w.len() != g.len() {
                return Some(format!("suggest count: want {} got {}", w.len(), g.len()));
            }
            for (ws, gs) in w.iter().zip(g) {
                let wm = ws.get("message").and_then(Value::as_str).unwrap_or("");
                if wm != gs.message {
                    return Some(format!("suggest message: want {wm:?} got {:?}", gs.message));
                }
                let want_out = ws.get("output").and_then(normalize_recorded_output);
                let got_out = Some(fix_output(source, &gs.fix));
                if !same_output(want_out.clone(), got_out.clone()) {
                    return Some(format!("suggest output: want {want_out:?} got {got_out:?}"));
                }
            }
        }
        (w, g) => return Some(format!("suggest: want {w:?} got {g:?}")),
    }
    None
}

fn group_by_uri<T>(items: impl Iterator<Item = (String, T)>) -> Vec<(String, Vec<T>)> {
    let mut groups: Vec<(String, Vec<T>)> = Vec::new();
    for (u, item) in items {
        match groups.iter_mut().find(|(k, _)| *k == u) {
            Some((_, l)) => l.push(item),
            None => groups.push((u, vec![item])),
        }
    }
    groups
}

/// The outcome of replaying one record.
pub enum RecordOutcome {
    Passed,
    Failed(String),
    Unreplayable(String),
    /// A needed check isn't implemented.
    NotImplemented(String),
}

/// Replays one record of a golden file.
pub fn replay_record(record: &Value, divergences: Divergences) -> RecordOutcome {
    let codes: Vec<&str> = record
        .get("checks")
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(Value::as_str)
        .collect();
    // `disabled-checks/test-checks.ts` is ported, so those specs replay too.
    let custom_ok = record.get("unreplayable").and_then(Value::as_str) == Some("custom check definition")
        && codes.iter().all(|c| TEST_REGISTRY.iter().any(|d| d.meta.code == *c));
    if let Some(reason) = record.get("unreplayable").and_then(Value::as_str) {
        if !custom_ok {
            return RecordOutcome::Unreplayable(reason.to_string());
        }
    }
    let registry: &'static [CheckDef] = if custom_ok { TEST_REGISTRY } else { REGISTRY };
    for code in &codes {
        let mut found = false;
        for def in registry.iter().filter(|d| d.meta.code == *code) {
            found = true;
            if !def.imp.is_implemented() {
                return RecordOutcome::NotImplemented((*code).to_string());
            }
        }
        if !found {
            return RecordOutcome::Unreplayable(format!("unknown check {code}"));
        }
    }
    let empty = Value::Object(Object::new());
    let recorded = record.get("deps").unwrap_or(&empty);
    let mode = match record.get("mode").and_then(Value::as_str) {
        Some("app") => Mode::App,
        _ => Mode::Theme,
    };
    let theme_desc = decode_theme(record.get("theme"));
    let sources: Vec<(String, String)> = theme_desc
        .iter()
        .map(|(k, v)| (uri::join(ROOT_URI, &[k]), v.as_str().unwrap_or("").to_string()))
        .collect();
    let theme: Theme<'_> = sources.iter().map(|(u, s)| to_source_code(u, s)).collect();
    let by_uri: HashMap<String, String> =
        theme.iter().map(|f: &SourceCode<'_>| (f.uri.clone(), f.source.to_string())).collect();

    let settings = record.get("settings").and_then(Value::as_object).cloned().unwrap_or_default();
    let checks: Vec<&'static str> = registry
        .iter()
        .filter(|d| codes.contains(&d.meta.code))
        .flat_map(|d| {
            // One config entry per definition; a code listed with one of two
            // definitions is told apart by the kind of what the record recorded.
            std::iter::once(d.meta.code)
        })
        .collect();
    let mut config = Config::new_for_test(mode, settings, checks, ROOT_URI.to_string(), None);
    config.divergences = divergences;
    let deps = MockDependencies::new(&theme, mode, &theme_desc, recorded);
    let output = check_with_registry(&theme, &config, &deps, registry);

    if let Some(err) = record.get("error") {
        let message = err.get("message").and_then(Value::as_str).unwrap_or("");
        return if output.errors.iter().any(|(_, _, e)| e.message == message) {
            RecordOutcome::Passed
        } else {
            RecordOutcome::Failed(format!("expected a thrown error {message:?}, got {:?}", output.errors))
        };
    }
    if let Some((code, u, e)) = output.errors.first() {
        return RecordOutcome::Failed(format!("{code} rejected on {u}: {}: {}", e.name, e.message));
    }

    let want = record.get("offenses").and_then(Value::as_array).unwrap_or(&[]);
    // Within a file, order must match. Across files it depends on upstream's microtask
    // interleaving, which the recording captures but the sync port doesn't promise.
    let want_groups = group_by_uri(
        want.iter().map(|o| (o.get("uri").and_then(Value::as_str).unwrap_or("").to_string(), o)),
    );
    let got_groups = group_by_uri(output.offenses.iter().map(|o| (o.uri.clone(), o)));
    for (u, wants) in &want_groups {
        let gots = got_groups.iter().find(|(k, _)| k == u).map_or(&[][..], |(_, l)| l.as_slice());
        if wants.len() != gots.len() {
            let got_msgs: Vec<&str> = gots.iter().map(|o| o.message.as_str()).collect();
            let want_msgs: Vec<&str> =
                wants.iter().map(|o| o.get("message").and_then(Value::as_str).unwrap_or("")).collect();
            return RecordOutcome::Failed(format!("{u}: want {want_msgs:?} got {got_msgs:?}"));
        }
        for (w, g) in wants.iter().zip(gots) {
            if let Some(d) = diff_offense(w, g, &by_uri) {
                return RecordOutcome::Failed(format!("{u}: {d}"));
            }
        }
    }
    if let Some((u, extra)) = got_groups.iter().find(|(k, _)| !want_groups.iter().any(|(w, _)| w == k)) {
        return RecordOutcome::Failed(format!("{u}: unexpected {:?}", extra[0].message));
    }

    // `fixed`: the autofix outcome per changed file.
    if let Some(fixed) = record.get("fixed").and_then(Value::as_object) {
        let error = fixed.contains_key("error");
        let mut got: Vec<(String, Result<String, String>)> = Vec::new();
        for file in &theme {
            let fixes: Vec<&Fix> =
                output.offenses.iter().filter(|o| o.uri == file.uri).filter_map(|o| o.fix.as_ref()).collect();
            if fixes.is_empty() {
                continue;
            }
            got.push((file.uri.clone(), fix_all(file.source, &fixes)));
        }
        if error {
            if !got.iter().any(|(_, r)| r.is_err()) {
                return RecordOutcome::Failed("fixed: expected autofix to throw".into());
            }
        } else {
            for (u, r) in &got {
                let rel = uri::relative(u, ROOT_URI);
                let source = by_uri.get(u).map_or("", String::as_str);
                let want_text: Option<String> = fixed.get(&rel).map(decode_source);
                match r {
                    Err(e) => return RecordOutcome::Failed(format!("fixed {rel}: threw {e}")),
                    Ok(text) => {
                        let expected = want_text.as_deref().unwrap_or(source);
                        if text != expected {
                            return RecordOutcome::Failed(format!("fixed {rel}: want {expected:?} got {text:?}"));
                        }
                    }
                }
            }
            for rel in fixed.keys() {
                let u = uri::join(ROOT_URI, &[rel]);
                if !got.iter().any(|(g, _)| *g == u) {
                    return RecordOutcome::Failed(format!("fixed {rel}: no fix produced"));
                }
            }
        }
    }
    RecordOutcome::Passed
}

/// The summary of a golden file.
#[derive(Debug, Default)]
pub struct GoldenSummary {
    pub check: String,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub unreplayable: usize,
    /// Records that need a check not yet ported.
    pub not_implemented: usize,
    pub failures: Vec<(String, String)>,
}

/// `lq debug-check-golden <file>`.
pub fn replay_file(path: &std::path::Path, divergences: Divergences) -> std::io::Result<GoldenSummary> {
    let text = std::fs::read_to_string(path)?;
    let mut summary = GoldenSummary {
        check: path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
        ..Default::default()
    };
    for line in text.lines().filter(|l| !l.is_empty()) {
        summary.total += 1;
        let Some(record) = lq_json::parse_json(line) else {
            summary.failed += 1;
            summary.failures.push(("<parse>".into(), "bad JSON line".into()));
            continue;
        };
        match replay_record(&record, divergences) {
            RecordOutcome::Passed => summary.passed += 1,
            RecordOutcome::Unreplayable(_) => summary.unreplayable += 1,
            RecordOutcome::NotImplemented(_) => summary.not_implemented += 1,
            RecordOutcome::Failed(diff) => {
                summary.failed += 1;
                let test = record.get("test").and_then(Value::as_str).unwrap_or("").to_string();
                summary.failures.push((test, diff));
            }
        }
    }
    Ok(summary)
}

