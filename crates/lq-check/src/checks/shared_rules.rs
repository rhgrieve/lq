//! Helpers for the rules batch of checks.

use crate::api::*;
use crate::check_utils::{is_attr, valued_attribute};
use crate::fixes::Corrector;
use crate::utils::file::{does_file_exceed_threshold, has_local_asset_size_exceeded_threshold, has_remote_asset_size_exceeded_threshold};
use lq_json::js::number_to_string;
use lq_json::regex::JsRegex;
use lq_json::validator::JsError;
use lq_json::Value;
use lq_syntax::{AttrNode, LiquidRawTag, Markup, Node};
use std::sync::OnceLock;

/// A `TypeError` from dereferencing `undefined` or `null`, ending the check on this file.
pub fn type_error(message: &str) -> JsError {
    JsError { name: "TypeError".into(), message: message.into() }
}

/// `node.attributes.filter(isValuedHtmlAttribute).find((attr) => isAttr(attr, name))`.
pub fn find_valued<'n, 's>(attributes: &'n [Node<'s>], name: &str) -> Option<&'n AttrNode<'s>> {
    attributes.iter().find_map(|a| valued_attribute(a).filter(|_| is_attr(a, name)))
}

/// The name of a `LiquidFilter` node.
pub fn filter_name<'n>(node: &'n Node<'_>) -> &'n str {
    match node {
        Node::LiquidFilter(f) => f.name,
        _ => "",
    }
}

/// The `LiquidVariable` of an output's markup.
pub fn output_variable<'n, 's>(node: &'n Node<'s>) -> Option<&'n lq_syntax::LiquidVariable<'s>> {
    let Node::LiquidVariableOutput(output) = node else { return None };
    let Markup::Node(markup) = &output.markup else { return None };
    match &**markup {
        Node::LiquidVariable(v) => Some(v),
        _ => None,
    }
}

/// `String(value)`.
pub fn js_to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number_to_string(*n),
        Value::String(s) => s.to_string(),
        Value::Array(items) => items
            .iter()
            .map(|v| if v.is_null() { String::new() } else { js_to_string(v) })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// `Number(string)`.
pub fn js_string_to_number(s: &str) -> f64 {
    let t = lq_syntax::js::trim(s);
    if t.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
        if let Some(digits) = t.strip_prefix(prefix) {
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            return digits.chars().fold(0.0, |n, c| n * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0)));
        }
    }
    let unsigned = t.strip_prefix(['+', '-']).unwrap_or(t);
    if unsigned == "Infinity" {
        return if t.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    // Rust also accepts `inf` and `nan`, whose letters a JS numeric literal can't have.
    if !unsigned.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-')) {
        return f64::NAN;
    }
    t.parse().unwrap_or(f64::NAN)
}

/// `Number(value)`.
pub fn js_to_number(value: &Value) -> f64 {
    match value {
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => *n,
        Value::String(s) => js_string_to_number(s),
        Value::Array(_) => js_string_to_number(&js_to_string(value)),
        Value::Object(_) => f64::NAN,
    }
}

/// The byte offset `units` UTF-16 code units after the byte offset `start`.
fn advance_utf16(source: &str, start: u32, units: usize) -> u32 {
    let mut count = 0;
    let rest = source.get(start as usize..).unwrap_or("");
    for (i, c) in rest.char_indices() {
        if count >= units {
            return start + i as u32;
        }
        count += c.len_utf16();
    }
    start + rest.len() as u32
}

/// `parseJSON(node.body.value)`, ahead of reading `property` from it. `None` after
/// `cx.fail` when the value is `undefined` or `null`, where that access throws.
pub fn parsed_schema_body(cx: &LiquidCx<'_, '_>, tag: &LiquidRawTag<'_>, property: &str) -> Option<Value> {
    let Node::RawMarkup(body) = &tag.body else { return None };
    match lq_json::parse_json(body.value) {
        Some(v) if !v.is_null() => Some(v),
        // `parseJSON` gives `undefined` for an empty body and `null` for `null`, neither of
        // which `isError` catches.
        _ if cx.divergence(Divergence::EmptySchema) => None,
        Some(_) => {
            cx.fail(type_error(&format!("Cannot read properties of null (reading '{property}')")));
            None
        }
        None => {
            cx.fail(type_error(&format!("Cannot read properties of undefined (reading '{property}')")));
            None
        }
    }
}

/// The body of `AssetSizeAppBlockCSS` and `AssetSizeAppBlockJavaScript`.
pub fn check_app_block_asset_size(cx: &LiquidCx<'_, '_>, tag: &LiquidRawTag<'_>, property: &str) {
    if tag.name != "schema" {
        return;
    }
    let Some(schema) = parsed_schema_body(cx, tag, property) else { return };
    let Some(asset) = schema.get(property).filter(|v| v.is_truthy()) else { return };
    let Node::RawMarkup(body) = &tag.body else { return };

    let name = js_to_string(asset);
    let relative_path = format!("assets/{name}");
    let threshold = cx.settings().f64("thresholdInBytes").unwrap_or(f64::NAN);

    let body_start = tag.body.position().start;
    let start = match body.value.find(name.as_str()) {
        Some(i) => body_start + i as u32,
        None => body_start.saturating_sub(1),
    };
    // `asset.length`: strings and arrays have one. Elsewhere it's `undefined` and
    // upstream's end index is `NaN`; this ends the range at its start instead.
    let units = match asset {
        Value::String(s) => crate::utils::js_len(s),
        Value::Array(items) => items.len(),
        _ => 0,
    };
    let end = advance_utf16(cx.source(), start, units);

    if !cx.does_file_exist(&relative_path) {
        cx.report(LiquidProblem::new(format!("'{name}' does not exist."), start, end));
        return;
    }
    let (exceeds, size) = does_file_exceed_threshold(cx, &relative_path, threshold);
    if exceeds {
        let threshold = number_to_string(threshold);
        cx.report(LiquidProblem::new(
            format!("The file size for '{name}' ({size} B) exceeds the configured threshold ({threshold} B)"),
            start,
            end,
        ));
    }
}

/// `<link href>` and `<script src>` of `AssetSizeCSS` and `AssetSizeJavaScript`.
pub fn check_asset_size_attribute(cx: &LiquidCx<'_, '_>, attr: &AttrNode<'_>, message: fn(&str) -> String) {
    let [value] = attr.value.as_slice() else { return };
    let threshold = cx.settings().f64("thresholdInBytes").unwrap_or(f64::NAN);
    let position = attr.attribute_position;
    // `/(https?:)?\/\//.test(url)`: the optional group makes this a substring test.
    if let Node::TextNode(text) = value {
        if text.value.contains("//") && has_remote_asset_size_exceeded_threshold(&text.value, threshold) {
            cx.report(LiquidProblem::new(message(&number_to_string(threshold)), position.start, position.end));
        }
    }
    let Some(variable) = output_variable(value) else { return };
    let Node::String(asset) = &variable.expression else { return };
    if variable.filters.len() == 1 && filter_name(&variable.filters[0]) == "asset_url" {
        let relative_path = format!("assets/{}", asset.value);
        if has_local_asset_size_exceeded_threshold(cx, &relative_path, threshold) {
            cx.report(LiquidProblem::new(message(&number_to_string(threshold)), position.start, position.end));
        }
    }
}

/// `{{ 'x' | asset_url | stylesheet_tag }}` of `AssetSizeCSS` and `AssetSizeJavaScript`.
pub fn check_asset_size_filter<'a>(
    cx: &LiquidCx<'a, '_>,
    tag_filter: &str,
    ancestors: &[&'a Node<'a>],
    message: fn(&str) -> String,
) {
    let Some(parent @ Node::LiquidVariable(variable)) = ancestors.last().copied() else { return };
    let Node::String(expression) = &variable.expression else { return };
    // `expression.value[0].length` throws on an empty string.
    if expression.value.is_empty() {
        if !cx.divergence(Divergence::EmptyAssetString) {
            cx.fail(type_error("Cannot read properties of undefined (reading 'length')"));
        }
        return;
    }
    let threshold = cx.settings().f64("thresholdInBytes").unwrap_or(f64::NAN);
    if variable.filters.len() == 1
        && expression.value.contains("//")
        && has_remote_asset_size_exceeded_threshold(expression.value, threshold)
    {
        let p = variable.expression.position();
        cx.report(LiquidProblem::new(message(&number_to_string(threshold)), p.start, p.end));
    }
    if variable.filters.len() == 2
        && filter_name(&variable.filters[0]) == "asset_url"
        && filter_name(&variable.filters[1]) == tag_filter
    {
        let relative_path = format!("assets/{}", expression.value);
        if has_local_asset_size_exceeded_threshold(cx, &relative_path, threshold) {
            let p = parent.position();
            cx.report(LiquidProblem::new(message(&number_to_string(threshold)), p.start, p.end));
        }
    }
}

const KILOBYTE: f64 = 1024.0;
const MEGABYTE: f64 = 1024.0 * KILOBYTE;

/// `MAX_FILE_SIZE_LIMITS` in order: `(key, bytes, is full-file measurement)`.
const MAX_FILE_SIZE_LIMITS: [(&str, f64, bool); 12] = [
    ("assets", 20.0 * MEGABYTE, true),
    ("locales", 1.5 * MEGABYTE, false),
    ("config/settings_data.json", 1.5 * MEGABYTE, false),
    ("config/settings_schema.json", 512.0 * KILOBYTE, false),
    ("templates/*.json", 512.0 * KILOBYTE, false),
    ("sections/*.json", 512.0 * KILOBYTE, false),
    ("blocks", 256.0 * KILOBYTE, false),
    ("config", 256.0 * KILOBYTE, false),
    ("layout", 256.0 * KILOBYTE, false),
    ("snippets", 256.0 * KILOBYTE, false),
    ("templates", 256.0 * KILOBYTE, false),
    ("sections", 256.0 * KILOBYTE, false),
];

fn matches_theme_path(relative_path: &str, theme_path: &str) -> bool {
    let bytes = relative_path.as_bytes();
    let len = theme_path.len();
    if relative_path == theme_path {
        return true;
    }
    if relative_path.starts_with(theme_path) && bytes.get(len) == Some(&b'/') {
        return true;
    }
    if relative_path.ends_with(theme_path) && bytes.len() > len && bytes[bytes.len() - len - 1] == b'/' {
        return true;
    }
    relative_path.match_indices(theme_path).any(|(i, _)| i > 0 && bytes[i - 1] == b'/' && bytes.get(i + len) == Some(&b'/'))
}

fn matches_theme_path_prefix(relative_path: &str, prefix: &str) -> bool {
    relative_path.starts_with(prefix)
        || relative_path.match_indices(prefix).any(|(i, _)| i > 0 && relative_path.as_bytes()[i - 1] == b'/')
}

fn max_file_size_limit(relative_path: &str) -> Option<(f64, bool)> {
    if relative_path == "_drafts" || relative_path.starts_with("_drafts/") {
        return None;
    }
    MAX_FILE_SIZE_LIMITS
        .iter()
        .find(|(key, _, _)| match key.strip_suffix("*.json") {
            Some(prefix) => {
                matches_theme_path_prefix(relative_path, prefix) && relative_path.to_lowercase().ends_with(".json")
            }
            None => matches_theme_path(relative_path, key),
        })
        .map(|&(_, bytes, full)| (bytes, full))
}

fn format_bytes(bytes: f64) -> String {
    if bytes % MEGABYTE == 0.0 {
        return format!("{} MB", number_to_string(bytes / MEGABYTE));
    }
    if bytes % KILOBYTE == 0.0 {
        return format!("{} KB", number_to_string(bytes / KILOBYTE));
    }
    format!("{} bytes", number_to_string(bytes))
}

fn schema_block_regex() -> &'static JsRegex {
    static RE: OnceLock<JsRegex> = OnceLock::new();
    RE.get_or_init(|| {
        JsRegex::new(r"{%-?\s*schema\s*-?%}[\s\S]*?{%-?\s*endschema\s*-?%}", "g").expect("valid regex")
    })
}

/// `MaxFileSize`'s `onCodePathStart`, for both its Liquid and JSON definitions.
pub fn check_max_file_size<C: Corrector>(cx: &Cx<'_, '_, C>) {
    let source = cx.source();
    let total = source.len() as f64;
    // Measured bytes never exceed the file's size, so most files stop here.
    if total <= 256.0 * KILOBYTE {
        return;
    }
    let relative_path = cx.to_relative_path(cx.uri());
    let Some((limit, full_file)) = max_file_size_limit(&relative_path) else { return };
    if total <= limit {
        return;
    }
    let measured = if full_file {
        total
    } else {
        let schema_bytes: usize = schema_block_regex().find_iter(source).iter().map(|m| m.as_str(source).len()).sum();
        total - schema_bytes as f64
    };
    if measured <= limit {
        return;
    }
    cx.report(Problem::new(
        format!("Theme file is too large. It is {} but the limit is {}.", format_bytes(measured), format_bytes(limit)),
        0,
        source.len() as u32,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(js_string_to_number(" 12 "), 12.0);
        assert_eq!(js_string_to_number(""), 0.0);
        assert_eq!(js_string_to_number("0x10"), 16.0);
        assert!(js_string_to_number("inf").is_nan());
        assert!(js_string_to_number("nan").is_nan());
        assert!(js_string_to_number("1a").is_nan());
        assert_eq!(js_string_to_number("-Infinity"), f64::NEG_INFINITY);
    }

    #[test]
    fn limits() {
        assert_eq!(max_file_size_limit("_drafts/snippets/a.liquid"), None);
        assert_eq!(max_file_size_limit("snippets/a.liquid"), Some((256.0 * KILOBYTE, false)));
        assert_eq!(max_file_size_limit("assets/a.css.liquid"), Some((20.0 * MEGABYTE, true)));
        assert_eq!(max_file_size_limit("templates/a.JSON"), Some((512.0 * KILOBYTE, false)));
        assert_eq!(max_file_size_limit("templates/a.liquid"), Some((256.0 * KILOBYTE, false)));
        assert_eq!(max_file_size_limit("other/x.liquid"), None);
        assert_eq!(format_bytes(1.5 * MEGABYTE), "1536 KB");
        assert_eq!(format_bytes(20.0 * MEGABYTE), "20 MB");
        assert_eq!(format_bytes(1234.0), "1234 bytes");
    }
}
