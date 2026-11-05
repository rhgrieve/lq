//! `to-schema.ts`: `{% schema %}` contents of blocks and sections.
//!
//! The JS `validSchema`/`parsed` are dynamic objects, so they are [`Value`]s here.

use crate::visitor::visit;
use lq_json::jsonc::parse_json_strict;
use lq_json::validator::{Diagnostic, JsError, JsonValidator, Severity};
use lq_json::{to_json_node, uri, JsonNode, Value};
use lq_syntax::{Markup, Node, ParseError};
use lq_theme::config::Mode;
use std::sync::OnceLock;

pub const NO_SCHEMA_TAG: &str = "No schema tag found";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeSchemaType {
    AppBlock,
    Block,
    Section,
}

/// `{% content_for 'block', type: type, id: id %}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticBlockDef {
    pub ty: String,
    pub id: String,
}

/// `isValidSchema`: what `validSchema` is checked with, on first use.
#[derive(Clone, Copy)]
pub enum IsValidSchema<'a> {
    /// `undefined`.
    Absent,
    Validator(&'a JsonValidator),
    /// `async () => true`, the test helper's.
    AlwaysValid,
}

/// A `ThemeSchema` (with `ThemeBlockSchema`, `SectionSchema`, `AppBlockSchema`).
pub struct ThemeSchema<'a> {
    pub ty: ThemeSchemaType,
    pub name: String,
    /// `offset`: byte offset of the JSON in the file. Add it to `ast` locations.
    pub offset: u32,
    /// `value`: the schema's text.
    pub value: &'a str,
    /// `ast`.
    pub ast: Result<JsonNode<'a>, JsError>,
    /// `parsed`. `Ok(None)` is `undefined`.
    pub parsed: Result<Option<Value>, JsError>,
    /// `staticBlockDefs` (empty for app blocks).
    pub static_block_defs: Vec<StaticBlockDef>,
    pub(crate) valid: OnceLock<Result<Option<Value>, JsError>>,
    /// `validate(uri, value)`, shared by `validSchema` and the `ValidSchema` check.
    pub(crate) validation: OnceLock<Result<Vec<Diagnostic>, JsError>>,
    pub(crate) validity: Validity<'a>,
    pub(crate) has_schema_node: bool,
    pub(crate) uri: String,
}

pub(crate) enum Validity<'a> {
    /// App block: no `validSchema`.
    None,
    Check(IsValidSchema<'a>),
    /// A recorded result.
    Fixed,
}

impl<'a> ThemeSchema<'a> {
    /// `validSchema`, computed once. `Err` is its `Error`. `None` for app blocks,
    /// which have no such field. `Ok(None)` is `undefined`.
    pub fn valid_schema(&self) -> Option<&Result<Option<Value>, JsError>> {
        if matches!(self.validity, Validity::None) {
            return None;
        }
        Some(self.valid.get_or_init(|| match &self.validity {
            Validity::Check(IsValidSchema::Absent) => Err(error("No JSON validator provided")),
            Validity::Check(_) if !self.has_schema_node => self.parsed.clone(),
            Validity::Check(IsValidSchema::AlwaysValid) => self.parsed.clone(),
            Validity::Check(IsValidSchema::Validator(_)) => {
                // A rejected validation (rare, a `$schema` key pointing nowhere) makes
                // upstream's whole `toSchema` promise reject, which a check sees as
                // its own failure. Here it reads as invalid; see PORTING.md.
                match self.shared_validation() {
                    Some(Ok(d)) if d.iter().all(|d| d.severity != Severity::Error) => self.parsed.clone(),
                    _ => Err(error("Invalid schema")),
                }
            }
            Validity::None | Validity::Fixed => Err(error("Invalid schema")),
        }))
    }

    /// The validator's result for this schema's text, computed once. `None` when no real
    /// validator backs this schema (no schema tag, test helper's `isValidSchema`, app blocks).
    pub fn shared_validation(&self) -> Option<&Result<Vec<Diagnostic>, JsError>> {
        let Validity::Check(IsValidSchema::Validator(v)) = &self.validity else { return None };
        if !self.has_schema_node {
            return None;
        }
        Some(self.validation.get_or_init(|| v.validate(&self.uri, self.value)))
    }

    /// Whether this schema was built from the file at `uri`.
    pub fn is_of_uri(&self, uri: &str) -> bool {
        self.uri == uri
    }

    /// `validSchema` when it is a usable value, the common early return of checks:
    /// `if (!validSchema || validSchema instanceof Error) return;`.
    pub fn valid_value(&self) -> Option<&Value> {
        match self.valid_schema()? {
            Ok(Some(v)) if v.is_truthy() => Some(v),
            _ => None,
        }
    }

    /// `hasNoSchemaTag(schema)`.
    pub fn has_no_schema_tag(&self) -> bool {
        matches!(&self.parsed, Err(e) if e.message == NO_SCHEMA_TAG)
    }
}

pub(crate) fn error(message: &str) -> JsError {
    JsError { name: "Error".into(), message: message.into() }
}

pub fn is_block(u: &str) -> bool {
    uri::dirname(u).ends_with("blocks")
}

pub fn is_section(u: &str) -> bool {
    uri::dirname(u).ends_with("sections")
}

pub fn is_snippet(u: &str) -> bool {
    uri::dirname(u).ends_with("snippets")
}

fn schema_node<'a>(root: Result<&'a Node<'a>, &ParseError>) -> Result<&'a Node<'a>, JsError> {
    let root = root.map_err(|e| JsError { name: e.name.clone(), message: e.message.clone() })?;
    let mut found = None;
    visit(root, &mut |node, _| {
        if found.is_none() {
            if let Node::LiquidRawTag(t) = node {
                if t.name == "schema" {
                    found = Some(node);
                }
            }
        }
    });
    found.ok_or_else(|| error(NO_SCHEMA_TAG))
}

fn static_block_defs<'a>(root: Result<&'a Node<'a>, &ParseError>) -> Vec<StaticBlockDef> {
    let Ok(root) = root else { return Vec::new() };
    let mut out = Vec::new();
    visit(root, &mut |node, _| {
        let Node::LiquidTag(tag) = node else { return };
        if tag.name != "content_for" {
            return;
        }
        let Markup::Node(markup) = &tag.markup else { return };
        let Node::ContentForMarkup(m) = &**markup else { return };
        let Node::String(content_type) = &m.content_for_type else { return };
        if content_type.value != "block" {
            return;
        }
        let arg = |name: &str| {
            m.args.iter().find_map(|a| match a {
                Node::NamedArgument(n) if n.name == name => Some(n),
                _ => None,
            })
        };
        let (Some(ty), Some(id)) = (arg("type"), arg("id")) else { return };
        let (Node::String(ty), Node::String(id)) = (&ty.value, &id.value) else { return };
        out.push(StaticBlockDef { ty: ty.value.to_string(), id: id.value.to_string() });
    });
    out
}

/// `parseJSON(value, undefined, isStrict)` as `parsed`.
pub(crate) fn to_parsed(value: &str) -> Result<Option<Value>, JsError> {
    parse_json_strict(value).map_err(|e| error(e.error.message()))
}

/// `toSchema(mode, uri, sourceCode, isValidSchema)` for a Liquid file. `None` when
/// the file isn't a block or section in that mode.
pub fn to_schema<'a>(
    mode: Mode,
    u: &str,
    root: Result<&'a Node<'a>, &ParseError>,
    is_valid: IsValidSchema<'a>,
) -> Option<ThemeSchema<'a>> {
    let block = is_block(u);
    let section = !block && is_section(u);
    let (ty, validity) = match (mode, block, section) {
        (Mode::App, true, _) => (ThemeSchemaType::AppBlock, Validity::None),
        (Mode::Theme, true, _) => (ThemeSchemaType::Block, Validity::Check(is_valid)),
        (Mode::Theme, _, true) => (ThemeSchemaType::Section, Validity::Check(is_valid)),
        _ => return None,
    };
    let node = schema_node(root);
    let (value, offset): (&'a str, u32) = match &node {
        Ok(Node::LiquidRawTag(t)) => {
            let text = match &t.body {
                Node::RawMarkup(b) => b.value,
                _ => "",
            };
            (text, t.block_start_position.end)
        }
        _ => ("", 0),
    };
    let has_schema_node = node.is_ok();
    let (parsed, ast) = match node {
        Ok(_) => (
            to_parsed(value),
            to_json_node(value).map_err(|_| error(lq_json::JsonParseFailure::MESSAGE)),
        ),
        Err(e) => (Err(e.clone()), Err(e)),
    };
    Some(ThemeSchema {
        ty,
        name: uri::basename(u, Some(".liquid")),
        offset,
        value,
        ast,
        parsed,
        static_block_defs: if ty == ThemeSchemaType::AppBlock { Vec::new() } else { static_block_defs(root) },
        valid: OnceLock::new(),
        validation: OnceLock::new(),
        validity,
        has_schema_node,
        uri: u.to_string(),
    })
}

/// Builds a schema from a recorded result (golden replay).
pub(crate) fn fixed_schema<'a>(
    ty: ThemeSchemaType,
    name: String,
    offset: u32,
    value: &'a str,
    parsed: Result<Option<Value>, JsError>,
    valid: Option<Result<Option<Value>, JsError>>,
    static_block_defs: Vec<StaticBlockDef>,
) -> ThemeSchema<'a> {
    let ast = to_json_node(value).map_err(|_| error(lq_json::JsonParseFailure::MESSAGE));
    let cell = OnceLock::new();
    let validity = match valid {
        Some(v) => {
            let _ = cell.set(v);
            Validity::Fixed
        }
        None => Validity::None,
    };
    ThemeSchema {
        ty,
        name,
        offset,
        value,
        ast,
        parsed,
        static_block_defs,
        valid: cell,
        validation: OnceLock::new(),
        validity,
        has_schema_node: true,
        uri: String::new(),
    }
}

/// `getSchemaFromJSON(context)`'s `parsed`: for the AST use `cx.file().json_ast()`.
pub fn parsed_json(source: &str) -> Option<Value> {
    lq_json::parse_json(source)
}
