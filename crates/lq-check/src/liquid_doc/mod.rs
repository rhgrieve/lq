//! `liquid-doc/*`.

pub mod arguments;

use crate::to_schema::{is_block, is_snippet};
use crate::visitor::visit;
use lq_json::js::is_js_whitespace;
use lq_syntax::js::trim;
use lq_syntax::{Node, NodeName};
use lq_theme::docs::ObjectEntry;
use std::collections::HashSet;

/// `LiquidDocParameter`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiquidDocParameter {
    pub name: String,
    pub description: Option<String>,
    pub ty: Option<String>,
    pub required: bool,
}

/// `DocContent`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocContent {
    pub parameters: Option<Vec<LiquidDocParameter>>,
    pub examples: Option<Vec<String>>,
    pub description: Option<String>,
}

/// `DocDefinition`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocDefinition {
    pub uri: String,
    pub liquid_doc: Option<DocContent>,
}

/// `hasLiquidDoc(snippet)`.
pub fn has_liquid_doc<'a>(root: &'a Node<'a>) -> bool {
    let mut found = false;
    visit(root, &mut |node, _| {
        if matches!(node, Node::LiquidRawTag(t) if t.name == "doc") {
            found = true;
        }
    });
    found
}

fn text_of<'n>(n: &'n Node<'_>) -> &'n str {
    match n {
        Node::TextNode(t) => &t.value,
        Node::String(s) => s.value,
        _ => "",
    }
}

/// `extractDocDefinition(uri, ast)`.
pub fn extract_doc_definition<'a>(uri: &str, root: &'a Node<'a>) -> DocDefinition {
    let mut has_doc = false;
    let mut parameters = Vec::new();
    let mut examples = Vec::new();
    let mut description: Option<String> = None;
    visit(root, &mut |node, _| match node {
        Node::LiquidRawTag(t) if t.name == "doc" => has_doc = true,
        Node::LiquidDocParamNode(n) => parameters.push(LiquidDocParameter {
            name: text_of(&n.param_name).to_string(),
            description: n.param_description.as_ref().map(|d| text_of(d).to_string()),
            ty: n.param_type.as_ref().map(|d| text_of(d).to_string()),
            required: n.required,
        }),
        Node::LiquidDocExampleNode(n) => {
            examples.push(handle_multiline_indentation(trim(text_of(&n.content))));
        }
        Node::LiquidDocDescriptionNode(n) if description.is_none() => {
            description = Some(handle_multiline_indentation(trim(text_of(&n.content))));
        }
        _ => {}
    });
    if !has_doc {
        return DocDefinition { uri: uri.to_string(), liquid_doc: None };
    }
    DocDefinition {
        uri: uri.to_string(),
        liquid_doc: Some(DocContent {
            parameters: (!parameters.is_empty()).then_some(parameters),
            examples: (!examples.is_empty()).then_some(examples),
            description,
        }),
    }
}

fn handle_multiline_indentation(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() <= 1 {
        return text.to_string();
    }
    let indent = |line: &str| line.chars().take_while(|c| is_js_whitespace(*c)).count();
    let min = lines[1..].iter().filter(|l| !trim(l).is_empty()).map(|l| indent(l)).min();
    let Some(min) = min else { return text.to_string() };
    let mut out = String::with_capacity(text.len());
    out.push_str(lines[0]);
    for line in &lines[1..] {
        out.push('\n');
        if trim(line).is_empty() {
            out.push_str(line);
        } else {
            let skip: usize = line.chars().take(min).map(char::len_utf8).sum();
            out.push_str(&line[skip..]);
        }
    }
    out
}

/// `StringLiteralType`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StringLiteralType {
    pub value: String,
    /// Keeps the quotes it was written with.
    pub raw: String,
}

/// `DocParamType`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocParamType {
    Named(String),
    Array(String),
    Literal(StringLiteralType),
    Union(Vec<StringLiteralType>),
}

/// `parseStringLiterals(type)`.
pub fn parse_string_literals(ty: &str) -> Option<Vec<StringLiteralType>> {
    if ty.contains(['\r', '\n']) {
        return None;
    }
    let b = ty.as_bytes();
    let mut literals = Vec::new();
    let mut pos = 0;
    let skip_ws = |pos: &mut usize| {
        while matches!(b.get(*pos), Some(b' ' | b'\t')) {
            *pos += 1;
        }
    };
    while pos < b.len() {
        skip_ws(&mut pos);
        let quote = *b.get(pos)?;
        if quote != b'\'' && quote != b'"' {
            return None;
        }
        let end = pos + 1 + ty[pos + 1..].find(quote as char)?;
        literals.push(StringLiteralType {
            value: ty[pos + 1..end].to_string(),
            raw: ty[pos..=end].to_string(),
        });
        pos = end + 1;
        skip_ws(&mut pos);
        if pos == b.len() {
            return Some(literals);
        }
        if b[pos] != b'|' {
            return None;
        }
        pos += 1;
    }
    None
}

/// `parseParamTypeSyntax(value)`: `/^([a-z_]+)(\[\])?$/`.
pub fn parse_param_type_syntax(value: &str) -> Option<(&str, bool)> {
    let (base, is_array) = match value.strip_suffix("[]") {
        Some(b) => (b, true),
        None => (value, false),
    };
    (!base.is_empty() && base.bytes().all(|c| c.is_ascii_lowercase() || c == b'_')).then_some((base, is_array))
}

/// `parseParamType(validParamTypes, value)`.
pub fn parse_param_type<'v>(valid: &HashSet<&str>, value: &'v str) -> Option<(&'v str, bool)> {
    let parsed = parse_param_type_syntax(value)?;
    valid.contains(parsed.0).then_some(parsed)
}

/// `parseDocParamType(validParamTypes, value)`.
pub fn parse_doc_param_type(valid: &HashSet<&str>, value: &str) -> Option<DocParamType> {
    if let Some(mut literals) = parse_string_literals(value) {
        return Some(if literals.len() == 1 {
            DocParamType::Literal(literals.remove(0))
        } else {
            DocParamType::Union(literals)
        });
    }
    let (name, is_array) = parse_param_type(valid, value)?;
    Some(if is_array { DocParamType::Array(name.to_string()) } else { DocParamType::Named(name.to_string()) })
}

/// `normalizeNamedParamType(value)`.
pub fn normalize_named_param_type(value: &str) -> Option<String> {
    let lower = value.to_lowercase();
    parse_param_type_syntax(&lower).is_some().then_some(lower)
}

/// `liquidDocType(type)`.
pub fn liquid_doc_type(ty: Option<&str>) -> Option<String> {
    match ty {
        Some(t) if !t.is_empty() => normalize_named_param_type(t),
        _ => None,
    }
}

/// `BasicParamTypes`.
pub const BASIC_PARAM_TYPES: [&str; 4] = ["string", "number", "boolean", "object"];

/// `getDefaultValueForType(type)`.
pub fn default_value_for_type(ty: Option<&str>) -> String {
    if let Some(literals) = ty.and_then(parse_string_literals) {
        return literals[0].raw.clone();
    }
    match ty.map(str::to_lowercase).as_deref() {
        Some("string") => "''".into(),
        Some("number") => "0".into(),
        Some("boolean") => "false".into(),
        _ => String::new(),
    }
}

/// `inferArgumentType(arg)`.
pub fn infer_argument_type(arg: &Node<'_>) -> &'static str {
    match arg {
        Node::String(_) => "string",
        Node::Number(_) => "number",
        Node::LiquidLiteral(_) => "boolean",
        _ => "object",
    }
}

/// `isTypeCompatible(expectedType, actualType)`.
pub fn is_type_compatible(expected: &str, actual: &str) -> bool {
    let e = expected.to_lowercase();
    e == "boolean" || e == actual
}

/// `ArgumentTypeCheck`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgumentTypeCheck {
    Compatible,
    Incompatible,
    Unchecked,
    NamedType(String),
}

fn literal_check(matches: bool) -> ArgumentTypeCheck {
    if matches {
        ArgumentTypeCheck::Compatible
    } else {
        ArgumentTypeCheck::Incompatible
    }
}

/// `checkArgumentType(expectedType, argument)`.
pub fn check_argument_type(expected: &str, argument: &Node<'_>) -> ArgumentTypeCheck {
    if matches!(argument, Node::VariableLookup(_)) {
        return ArgumentTypeCheck::Unchecked;
    }
    if let Some(literals) = parse_string_literals(expected) {
        return literal_check(match argument {
            Node::String(s) => literals.iter().any(|l| l.value == s.value),
            _ => false,
        });
    }
    if matches!(argument, Node::BlockArrayLiteral(_)) {
        return ArgumentTypeCheck::Unchecked;
    }
    let Some(ty) = normalize_named_param_type(expected) else {
        return ArgumentTypeCheck::Unchecked;
    };
    if !BASIC_PARAM_TYPES.contains(&ty.as_str()) {
        return ArgumentTypeCheck::NamedType(ty);
    }
    literal_check(is_type_compatible(&ty, infer_argument_type(argument)))
}

/// `getArgumentTypeMismatchMessage(name, expectedType, argument)`. `source` is the
/// file the argument came from.
pub fn argument_type_mismatch_message(name: &str, expected: &str, argument: &Node<'_>, source: &str) -> String {
    if parse_string_literals(expected).is_some() {
        let p = argument.position();
        let actual = lq_syntax::js::slice(source, p.start, p.end);
        return format!(
            "Invalid value for argument '{name}': expected {}, got {actual}",
            trim(expected)
        );
    }
    let actual = if matches!(argument, Node::BlockArrayLiteral(_)) { "array" } else { infer_argument_type(argument) };
    format!("Type mismatch for argument '{name}': expected {}, got {actual}", expected.to_lowercase())
}

/// `filePathSupportsLiquidDoc(uri)`.
pub fn file_path_supports_liquid_doc(uri: &str) -> bool {
    is_snippet(uri) || is_block(uri)
}

/// The keys of `getValidParamTypes(objectEntries)`. Upstream's values are docs text.
pub fn valid_param_types(objects: &[ObjectEntry]) -> HashSet<&str> {
    let mut set: HashSet<&str> = BASIC_PARAM_TYPES.iter().copied().collect();
    set.extend(objects.iter().map(|o| o.name));
    set
}

/// A string-valued `name` as written on a tag's markup argument.
pub fn node_name_str<'n>(node: &'n Node<'_>) -> Option<&'n str> {
    match node.name()? {
        NodeName::Str(s) => Some(s),
        _ => None,
    }
}
