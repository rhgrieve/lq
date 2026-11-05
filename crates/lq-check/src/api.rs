//! The API checks are written against. See `PORTING.md`.

use crate::fixes::{Corrector, Fix, JsonCorrector, StringCorrector};
use crate::run::{FileState, RunContext};
use crate::source::SourceCode;
use lq_json::validator::{Diagnostic, JsError};
use lq_json::{JsonNode, Object, Value};
use lq_syntax::Node;
use lq_theme::config::{Mode, SchemaProp};
pub use lq_theme::divergence::{Divergence, Divergences};
use lq_theme::docs::ThemeDocset;
use std::marker::PhantomData;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceCodeType {
    Json,
    LiquidHtml,
}

impl SourceCodeType {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceCodeType::Json => "JSON",
            SourceCodeType::LiquidHtml => "LiquidHtml",
        }
    }
}

/// Lower is more severe, as upstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error = 0,
    Warning = 1,
    Info = 2,
}

impl Severity {
    pub fn from_number(n: f64) -> Option<Severity> {
        match n as i64 {
            0 => Some(Severity::Error),
            1 => Some(Severity::Warning),
            2 => Some(Severity::Info),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigTarget {
    All,
    Recommended,
    ThemeAppExtension,
}

/// `check.meta`.
pub struct CheckMeta {
    pub code: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub recommended: bool,
    pub url: Option<&'static str>,
    pub severity: Severity,
    pub kind: SourceCodeType,
    pub aliases: &'static [&'static str],
    pub targets: &'static [ConfigTarget],
    /// Settings schema with defaults, as `(key, prop)`.
    pub schema: &'static [(&'static str, SchemaProp)],
    pub deprecated: bool,
}

/// A registry entry.
pub struct CheckDef {
    pub meta: CheckMeta,
    pub imp: CheckImpl,
}

pub type LiquidCreate =
    for<'a, 'f> fn(&LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>>;
pub type JsonCreate = for<'a, 'f> fn(&JsonCx<'a, 'f>) -> Option<Box<dyn JsonCheck<'a> + 'a>>;

/// What a check does. Each `checks/<name>.rs` exports `pub const IMPL: CheckImpl`.
#[derive(Clone, Copy)]
pub enum CheckImpl {
    NotImplemented,
    Liquid { create: LiquidCreate, enter: NodeMask, exit: NodeMask },
    Json { create: JsonCreate, enter: JsonMask, exit: JsonMask },
}

impl CheckImpl {
    pub const fn is_implemented(&self) -> bool {
        !matches!(self, CheckImpl::NotImplemented)
    }
}

macro_rules! node_kinds {
    ($($v:ident),* $(,)?) => {
        /// The `type` of a Liquid AST node, for dispatch masks.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u8)]
        pub enum NodeKind { $($v),* }

        impl NodeKind {
            pub const COUNT: usize = [$(NodeKind::$v),*].len();

            #[inline]
            pub fn of(node: &Node<'_>) -> NodeKind {
                match node { $(Node::$v(_) => NodeKind::$v),* }
            }
        }
    };
}

node_kinds!(
    Document, YamlFrontmatter, LiquidRawTag, LiquidTag, LiquidBranch, LiquidVariableOutput,
    HtmlSelfClosingElement, HtmlVoidElement, HtmlDoctype, HtmlComment, HtmlElement,
    HtmlDanglingMarkerClose, HtmlRawNode, AttrSingleQuoted, AttrDoubleQuoted, AttrUnquoted,
    AttrEmpty, TextNode, LiquidVariable, LiquidFilter, NamedArgument, LiquidLiteral,
    BooleanExpression, String, Number, Range, VariableLookup, Comparison, LogicalExpression,
    AssignMarkup, BlockMarkup, ContentForMarkup, CycleMarkup, ForMarkup, PaginateMarkup,
    RawMarkup, RenderMarkup, SectionMarkup, RenderVariableExpression, RenderAliasExpression,
    LiquidDocParamNode, LiquidDocDescriptionNode, LiquidDocExampleNode, LiquidDocPromptNode,
    LiquidErrorNode, BlockArrayLiteral,
);

/// The set of node kinds a Liquid check wants `enter` or `exit` for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct NodeMask(pub u64);

impl NodeMask {
    pub const NONE: NodeMask = NodeMask(0);
    pub const ALL: NodeMask = NodeMask(u64::MAX);

    pub const fn of(kinds: &[NodeKind]) -> NodeMask {
        let mut m = 0u64;
        let mut i = 0;
        while i < kinds.len() {
            m |= 1 << kinds[i] as u8;
            i += 1;
        }
        NodeMask(m)
    }

    #[inline]
    pub const fn contains(self, kind: NodeKind) -> bool {
        self.0 & (1 << kind as u8) != 0
    }
}

/// The `type` of a JSON AST node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum JsonKind {
    Object,
    Array,
    Property,
    Identifier,
    Literal,
}

impl JsonKind {
    pub const COUNT: usize = 5;

    #[inline]
    pub fn of(node: &JsonNode<'_>) -> JsonKind {
        match node {
            JsonNode::Object { .. } => JsonKind::Object,
            JsonNode::Array { .. } => JsonKind::Array,
            JsonNode::Property { .. } => JsonKind::Property,
            JsonNode::Identifier { .. } => JsonKind::Identifier,
            JsonNode::Literal { .. } => JsonKind::Literal,
        }
    }
}

/// The set of JSON node kinds a JSON check wants `enter` or `exit` for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct JsonMask(pub u8);

impl JsonMask {
    pub const NONE: JsonMask = JsonMask(0);

    pub const fn of(kinds: &[JsonKind]) -> JsonMask {
        let mut m = 0u8;
        let mut i = 0;
        while i < kinds.len() {
            m |= 1 << kinds[i] as u8;
            i += 1;
        }
        JsonMask(m)
    }

    #[inline]
    pub const fn contains(self, kind: JsonKind) -> bool {
        self.0 & (1 << kind as u8) != 0
    }
}

/// A Liquid check: one instance per file, so fields are per-file state.
///
/// `enter`/`exit` run only for kinds in the masks of the [`CheckImpl`]. Upstream's
/// `X:exit` runs right after `X`, *before* the node's children are visited.
#[allow(unused_variables)]
pub trait LiquidCheck<'a> {
    /// Runs even when the file doesn't parse (`cx.file().liquid_ast()` is `Err`).
    fn on_code_path_start(&mut self, cx: &LiquidCx<'a, '_>) {}
    fn enter(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {}
    fn exit(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {}
    /// Runs only when the file parsed.
    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {}
}

/// A JSON check: one instance per file. Visiting order is upstream's `visitJSON`.
#[allow(unused_variables)]
pub trait JsonCheck<'a> {
    /// Runs even when the file doesn't parse (`cx.file().json_ast()` is `Err`).
    fn on_code_path_start(&mut self, cx: &JsonCx<'a, '_>) {}
    fn enter(&mut self, node: &'a JsonNode<'a>, ancestors: &[&'a JsonNode<'a>], cx: &JsonCx<'a, '_>) {}
    fn exit(&mut self, node: &'a JsonNode<'a>, ancestors: &[&'a JsonNode<'a>], cx: &JsonCx<'a, '_>) {}
    /// Runs only when the file parsed.
    fn on_code_path_end(&mut self, cx: &JsonCx<'a, '_>) {}
}

/// A problem to report. `start` and `end` are byte offsets in the file; `C` is the
/// corrector kind ([`StringCorrector`] for Liquid, [`JsonCorrector`] for JSON).
pub struct Problem<C> {
    pub message: String,
    pub start: u32,
    pub end: u32,
    pub fix: Option<Fix>,
    pub suggest: Option<Vec<Suggestion>>,
    pub hints: Hints,
    _corrector: PhantomData<C>,
}

/// A problem in a Liquid file.
pub type LiquidProblem = Problem<StringCorrector>;
/// A problem in a JSON file.
pub type JsonProblem = Problem<JsonCorrector>;

/// Help for the person reading a report. `lq`'s own, not upstream's: it never changes which
/// offenses exist or their messages, positions and fixes, and `--output json` leaves it out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hints {
    /// The short text beside the underline.
    pub label: Option<String>,
    pub notes: Vec<Note>,
    /// The problem is with the file as a whole, whatever its span says.
    pub file_level: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub message: String,
    /// Where it points, when that's somewhere else: a file and a byte range in it.
    pub related: Option<Related>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Related {
    pub uri: String,
    pub start: u32,
    pub end: u32,
}

/// A fix that can't be applied automatically; `fix` is data, already collected.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub message: String,
    pub fix: Fix,
}

impl<C: Corrector> Problem<C> {
    pub fn new(message: impl Into<String>, start: u32, end: u32) -> Self {
        Problem { message: message.into(), start, end, fix: None, suggest: None, hints: Hints::default(), _corrector: PhantomData }
    }

    /// `problem.fix`: a safe autofix. The closure runs now and records the changes.
    pub fn fix(mut self, f: impl FnOnce(&mut C)) -> Self {
        let mut c = C::default();
        f(&mut c);
        self.fix = Some(c.into_fix());
        self
    }

    /// Adds one entry to `problem.suggest`.
    pub fn suggest(mut self, message: impl Into<String>, f: impl FnOnce(&mut C)) -> Self {
        let mut c = C::default();
        f(&mut c);
        self.suggest
            .get_or_insert_with(Vec::new)
            .push(Suggestion { message: message.into(), fix: c.into_fix() });
        self
    }

    /// Adds hints only when the report can show them. See [`Cx::wants_hints`].
    pub fn hints_if(self, wanted: bool, add: impl FnOnce(Self) -> Self) -> Self {
        if wanted {
            add(self)
        } else {
            self
        }
    }

    /// The problem is with the file as a whole, so there's nothing in it to point at.
    pub fn file_level(mut self) -> Self {
        self.hints.file_level = true;
        self
    }

    /// The short text beside the underline.
    pub fn label(mut self, text: impl Into<String>) -> Self {
        self.hints.label = Some(text.into());
        self
    }

    /// A line of help for the reader.
    pub fn note(mut self, text: impl Into<String>) -> Self {
        self.hints.notes.push(Note { message: text.into(), related: None });
        self
    }

    /// A line of help, if there is one.
    pub fn note_opt(self, text: Option<String>) -> Self {
        match text {
            Some(text) => self.note(text),
            None => self,
        }
    }

    /// A line of help that points at a byte range of another file.
    pub fn note_at(mut self, text: impl Into<String>, uri: impl Into<String>, start: u32, end: u32) -> Self {
        let related = Some(Related { uri: uri.into(), start, end });
        self.hints.notes.push(Note { message: text.into(), related });
        self
    }

    /// `suggest: []`: an empty list, as opposed to none at all.
    pub fn with_empty_suggestions(mut self) -> Self {
        self.suggest.get_or_insert_with(Vec::new);
        self
    }
}

/// `Settings<S>` of `createSettings`: each schema key is the configured value
/// (unless `null`/absent) or the default.
#[derive(Clone, Debug, Default)]
pub struct Settings(pub Object);

impl Settings {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }
    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }
    pub fn f64(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(Value::as_f64)
    }
    pub fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(Value::as_bool)
    }
    /// The strings of an array setting, ignoring other items.
    pub fn strs(&self, key: &str) -> Vec<&str> {
        self.get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }
}

/// The `context` of a check: everything upstream's `Context` has. One per check
/// per file. Upstream's async dependencies are plain calls here, computed lazily
/// and shared across files.
pub struct Cx<'a, 'f, C> {
    pub(crate) run: &'f RunContext<'a>,
    pub(crate) file: &'a SourceCode<'a>,
    pub(crate) def: &'static CheckDef,
    pub(crate) index: u16,
    pub(crate) settings: &'f Settings,
    pub(crate) severity: Severity,
    pub(crate) state: &'f FileState<'a>,
    pub(crate) _corrector: PhantomData<C>,
}

/// The context of a Liquid check.
pub type LiquidCx<'a, 'f> = Cx<'a, 'f, StringCorrector>;
/// The context of a JSON check.
pub type JsonCx<'a, 'f> = Cx<'a, 'f, JsonCorrector>;

impl<'a, 'f, C: Corrector> Cx<'a, 'f, C> {
    /// `context.report(problem)`.
    pub fn report(&self, problem: Problem<C>) {
        self.state.report(self.def, self.index, self.severity, self.file, problem.message, problem.start, problem.end, problem.fix, problem.suggest, problem.hints);
    }
}

impl<'a, 'f, C> Cx<'a, 'f, C> {
    /// Whether the fix for this upstream bug is on. See `lq_theme::divergence`.
    #[inline]
    pub fn divergence(&self, d: Divergence) -> bool {
        self.run.config.divergences.contains(d)
    }

    /// Whether the report can show hints: when false, don't spend time working them out.
    #[inline]
    pub fn wants_hints(&self) -> bool {
        self.run.config.hints
    }

    /// `context.file`.
    pub fn file(&self) -> &'a SourceCode<'a> {
        self.file
    }

    /// `context.file.source`.
    pub fn source(&self) -> &'a str {
        self.file.source
    }

    /// `context.file.uri`.
    pub fn uri(&self) -> &'a str {
        &self.file.uri
    }

    /// `context.settings`.
    pub fn settings(&self) -> &Settings {
        self.settings
    }

    /// `context.mode`.
    pub fn mode(&self) -> Mode {
        self.run.config.context
    }

    pub fn root_uri(&self) -> &'a str {
        &self.run.config.root_uri
    }

    /// `context.toUri(relativePath)`.
    pub fn to_uri(&self, relative_path: &str) -> String {
        self.run.to_uri(relative_path)
    }

    /// `context.toRelativePath(uri)`.
    pub fn to_relative_path(&self, uri: &str) -> String {
        lq_json::uri::relative(uri, &self.run.config.root_uri)
    }

    /// `context.fileExists(uri)`.
    pub fn file_exists(&self, uri: &str) -> bool {
        self.run.deps.fs().stat(uri).is_ok()
    }

    /// `context.fileSize(uri)`: 0 on failure. In JS string length units for the
    /// mock file system, bytes on disk.
    pub fn file_size(&self, uri: &str) -> u64 {
        self.run.deps.fs().stat(uri).map_or(0, |s| s.size)
    }

    /// `doesFileExist(context, relativePath)`.
    pub fn does_file_exist(&self, relative_path: &str) -> bool {
        self.file_exists(&self.to_uri(relative_path))
    }

    /// `context.fs`.
    pub fn fs(&self) -> &'a dyn crate::fs::FileSystem {
        self.run.deps.fs()
    }

    /// `context.themeDocset`, augmented. `None` is `undefined`.
    pub fn docset(&self) -> Option<&dyn ThemeDocset> {
        self.run.docset()
    }

    /// Whether `context.validateJSON` is defined.
    pub fn has_validate_json(&self) -> bool {
        self.run.deps.has_json_validator()
    }

    /// `context.validateJSON(uri, json)`. `None` when it is undefined. Offsets are
    /// bytes into `json`.
    pub fn validate_json(&self, uri: &str, json: &str) -> Option<Result<Vec<Diagnostic>, JsError>> {
        self.run.deps.json_validator().map(|v| v.validate(uri, json))
    }

    /// `context.getDefaultLocale()`.
    pub fn default_locale(&self) -> &str {
        self.run.default_locale()
    }

    /// `context.getDefaultSchemaLocale()`.
    pub fn default_schema_locale(&self) -> &str {
        self.run.default_schema_locale()
    }

    /// `context.getDefaultTranslations()`. `None` is `undefined`, which upstream
    /// can produce for an empty locale file.
    pub fn default_translations(&self) -> Option<&Value> {
        self.run.default_translations()
    }

    /// `context.getDefaultSchemaTranslations()`.
    pub fn default_schema_translations(&self) -> Option<&Value> {
        self.run.default_schema_translations()
    }

    /// `context.getMetafieldDefinitions(rootUri)`.
    pub fn metafield_definitions(&self) -> &crate::context_utils::MetafieldDefinitionMap {
        self.run.metafield_definitions()
    }

    /// `context.getBlockSchema?.(name)`.
    pub fn block_schema(&self, name: &str) -> Option<&'a crate::to_schema::ThemeSchema<'a>> {
        self.run.deps.block_schema(name)
    }

    /// `context.getSectionSchema?.(name)`.
    pub fn section_schema(&self, name: &str) -> Option<&'a crate::to_schema::ThemeSchema<'a>> {
        self.run.deps.section_schema(name)
    }

    /// `context.getAppBlockSchema?.(name)`.
    pub fn app_block_schema(&self, name: &str) -> Option<&'a crate::to_schema::ThemeSchema<'a>> {
        self.run.deps.app_block_schema(name)
    }

    /// `getSchema(context)`: the schema of this file when it's a block or section.
    pub fn schema(&self) -> Option<&'a crate::to_schema::ThemeSchema<'a>> {
        let name = lq_json::uri::basename(self.uri(), Some(".liquid"));
        if crate::to_schema::is_block(self.uri()) {
            self.block_schema(&name)
        } else if crate::to_schema::is_section(self.uri()) {
            self.section_schema(&name)
        } else {
            None
        }
    }

    /// `context.getDocDefinition?.(relativePath)`.
    pub fn doc_definition(&self, relative_path: &str) -> Option<&'a crate::liquid_doc::DocDefinition> {
        self.run.deps.doc_definition(relative_path)
    }

    /// `context.getBlockParameters(blockName)`.
    pub fn block_parameters(&self, block_name: &str) -> Option<std::sync::Arc<crate::block_parameters::BlockParameters<'a>>> {
        self.run.block_parameters(block_name)
    }

    /// Whether `context.getCSSClassesForURI` is defined.
    pub fn has_css_classes(&self) -> bool {
        self.run.deps.has_css_classes()
    }

    /// `context.getCSSClassesForURI(uri)`.
    pub fn css_classes_for_uri(&self, uri: &str) -> std::sync::Arc<std::collections::BTreeSet<String>> {
        self.run.deps.css_classes_for_uri(uri)
    }

    /// Whether `context.getReferences` is defined.
    ///
    /// With [`Divergence::GraphFailure`] on, a graph that failed to build is undefined, not
    /// empty, so nothing is concluded from it.
    pub fn has_references(&self) -> bool {
        self.run.deps.has_references() && !self.graph_failed()
    }

    fn graph_failed(&self) -> bool {
        self.divergence(Divergence::GraphFailure) && self.run.deps.graph_error().is_some()
    }

    /// `context.getReferences(uri)`.
    pub fn references(&self, uri: &str) -> Vec<crate::graph::Reference> {
        self.run.deps.references(uri)
    }

    /// Whether `context.getDependencies` is defined.
    pub fn has_dependencies(&self) -> bool {
        self.run.deps.has_dependencies() && !self.graph_failed()
    }

    /// `context.getDependencies(uri)`.
    pub fn dependencies(&self, uri: &str) -> Vec<crate::graph::Reference> {
        self.run.deps.dependencies(uri)
    }

    /// For a handler whose `await` takes `rounds` more microtask rounds than a plain
    /// call: its reports this call sort that much later against other checks.
    pub(crate) fn delay_reports(&self, rounds: u32) {
        self.state.delay(rounds);
    }

    /// Upstream's rejected promise: stops this check on this file. What it already
    /// reported stays. Goes to `config.onError`, silent by default.
    pub fn fail(&self, error: JsError) {
        self.state.fail(self.index, error);
    }
}
