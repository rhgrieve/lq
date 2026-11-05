//! `LiquidHTMLSyntaxError`: parse errors, plus markup the parser only accepts by
//! falling back to a string, or that lax Liquid parses differently than it reads.
//!
//! `checks/*.ts` upstream are the sections below. Offsets in the regexes' results are
//! bytes here, UTF-16 there; both index the same string, so the arithmetic carries over.

use crate::api::*;
use crate::check_utils::{is_loop_tag, is_within_unparsed_raw_tag};
use lq_json::regex::JsRegex;
use lq_syntax::js::{slice, trim, trim_end};
use lq_syntax::{LiquidTag, LiquidVariable, Markup, Node, ParseError, PARSING_ERROR_NAME};
use std::sync::OnceLock;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[
        NodeKind::BooleanExpression,
        NodeKind::LiquidTag,
        NodeKind::LiquidBranch,
        NodeKind::LiquidVariableOutput,
    ]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(LiquidHtmlSyntaxError))
}

struct LiquidHtmlSyntaxError;

impl<'a> LiquidCheck<'a> for LiquidHtmlSyntaxError {
    fn on_code_path_start(&mut self, cx: &LiquidCx<'a, '_>) {
        if let Err(error) = cx.file().liquid_ast() {
            report_parse_error(error, cx);
        }
    }

    fn enter(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        if is_within_unparsed_raw_tag(ancestors) {
            return;
        }
        match node {
            Node::BooleanExpression(_) => detect_invalid_boolean_expression(node, cx),
            Node::LiquidTag(tag) => {
                detect_multiple_assign_values(tag, cx);
                if tag.name == "echo" {
                    detect_invalid_echo_value(&tag.markup, node.position().start, cx);
                }
                detect_invalid_loop_range(tag, node.position().start, cx);
                detect_invalid_loop_arguments(tag, node.position().start, cx);
                // Their fixers both consume the whole markup, so only one may run.
                if !detect_unsupported_parenthesis(tag, node.position().start, cx) {
                    detect_invalid_conditional_node(Some(tag.name), &tag.markup, tag.block_start_position, cx);
                }
                match (tag.name, &tag.markup) {
                    ("echo" | "assign", Markup::Str(markup)) => {
                        let start = node.position().start;
                        detect_invalid_filter_name_in_markup(markup, start, cx);
                        detect_invalid_pipe_syntax(markup, start, cx);
                    }
                    ("echo", Markup::Node(n)) => {
                        if let Node::LiquidVariable(variable) = &**n {
                            detect_invalid_filter_name_in_filters(variable, n.position().end, cx);
                        }
                    }
                    ("assign", Markup::Node(n)) => {
                        if let Node::AssignMarkup(assign) = &**n {
                            if let Node::LiquidVariable(variable) = &assign.value {
                                detect_invalid_filter_name_in_filters(variable, assign.value.position().end, cx);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Node::LiquidBranch(branch) => {
                detect_invalid_conditional_node(branch.name, &branch.markup, branch.block_start_position, cx);
            }
            Node::LiquidVariableOutput(output) => {
                let start = node.position().start;
                match &output.markup {
                    Markup::Str(markup) => {
                        detect_invalid_filter_name_in_markup(markup, start, cx);
                        detect_invalid_pipe_syntax(markup, start, cx);
                    }
                    Markup::Node(n) => {
                        if let Node::LiquidVariable(variable) = &**n {
                            detect_invalid_filter_name_in_filters(variable, n.position().end, cx);
                        }
                    }
                    _ => {}
                }
                detect_invalid_echo_value(&output.markup, start, cx);
            }
            _ => {}
        }
    }
}

macro_rules! regex {
    ($name:ident, $flags:literal, $pattern:expr) => {
        fn $name() -> &'static JsRegex {
            static RE: OnceLock<JsRegex> = OnceLock::new();
            RE.get_or_init(|| JsRegex::new(&$pattern, $flags).expect(stringify!($name)))
        }
    };
}

// `index.ts`.

regex!(line_col_re, "", r"Line \d+, col \d+:\s+");
regex!(not_re, "", r#"(?!<expected ".+",) not .*"#);

fn report_parse_error<'a>(error: &ParseError, cx: &LiquidCx<'a, '_>) {
    let source = cx.source();
    let loc = error.loc.filter(|_| error.name == PARSING_ERROR_NAME);
    let Some(loc) = loc else {
        cx.report(LiquidProblem::new(error.message.clone(), 0, source.len() as u32));
        return;
    };
    let start = line_col_to_byte(source, loc.start.line, loc.start.column);
    let mut end = line_col_to_byte(source, loc.end.line, loc.end.column);
    if start == end {
        end = next_utf16_unit(source, end);
    }
    let highlight = lossy_slice(source, start, end);
    cx.report(LiquidProblem::new(clean_error_message(&error.message, &highlight), start, end));
}

/// `getOffset(source, line, column)`, 1-origin with columns in UTF-16 units, as a byte
/// offset. The second unit of an astral char is two bytes into it, which is where the
/// parser's clamp to `source.length - 1` lands when the source ends with one, and what
/// the engine reports as that unit. Parse errors always resolve inside the source, so
/// the `line-column` package's `-1` can't come up. An empty source never fails.
fn line_col_to_byte(source: &str, line: u32, column: u32) -> u32 {
    let mut start = 0usize;
    for _ in 1..line {
        match source[start..].find('\n') {
            Some(i) => start += i + 1,
            None => break,
        }
    }
    let mut units = column.saturating_sub(1) as usize;
    let mut at = start;
    for c in source[start..].chars() {
        if units == 0 {
            break;
        }
        if units < c.len_utf16() {
            return (at + 2) as u32;
        }
        units -= c.len_utf16();
        at += c.len_utf8();
    }
    at as u32
}

/// `index + 1` in UTF-16 units, in the convention of [`line_col_to_byte`]. Past the
/// end it keeps counting, as the index does.
fn next_utf16_unit(source: &str, byte: u32) -> u32 {
    let at = byte as usize;
    if !source.is_char_boundary(at) {
        // In the middle of an astral char: its end.
        return byte + 2;
    }
    match source[at.min(source.len())..].chars().next() {
        Some(c) if c.len_utf16() == 2 => byte + 2,
        Some(c) => byte + c.len_utf8() as u32,
        None => byte + 1,
    }
}

/// `source.slice(start, end)`. Half an astral char is a lone surrogate upstream, which
/// a Rust string can't hold; it prints as U+FFFD either way.
fn lossy_slice(source: &str, start: u32, end: u32) -> String {
    let (start, end) = (start as usize, (end as usize).min(source.len()));
    if start >= end {
        return String::new();
    }
    let mut out = String::new();
    let mut from = start;
    if !source.is_char_boundary(start) {
        out.push('\u{FFFD}');
        from = start + 2;
    }
    let mut to = end;
    let split_end = !source.is_char_boundary(end);
    if split_end {
        to = end - 2;
    }
    if from < to {
        out.push_str(&source[from..to]);
    }
    if split_end {
        out.push('\u{FFFD}');
    }
    out
}

/// `cleanErrorMessage`.
fn clean_error_message(message: &str, highlight: &str) -> String {
    let mut out = message.to_string();
    if let Some(m) = line_col_re().exec_at(&out, 0) {
        out.replace_range(m.start()..m.end(), "SyntaxError: ");
    }
    if let Some(m) = not_re().exec_at(&out, 0) {
        let (matched, before, after) = (&out[m.start()..m.end()], &out[..m.start()], &out[m.end()..]);
        let replacement = get_substitution(highlight, matched, before, after);
        out = format!("{before} not \"{replacement}\"{after}");
    }
    out
}

/// The `$` patterns of a `String.prototype.replace` replacement, for a regex without
/// capture groups (where `$1` stays literal). `highlight` is source text, so it can
/// hold them; the ` not "` around it can't.
fn get_substitution(template: &str, matched: &str, before: &str, after: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        match tail.as_bytes().first() {
            Some(b'$') => out.push('$'),
            Some(b'&') => out.push_str(matched),
            Some(b'`') => out.push_str(before),
            Some(b'\'') => out.push_str(after),
            _ => {
                out.push('$');
                rest = tail;
                continue;
            }
        }
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

// `checks/utils.ts`.

const INVALID_SYNTAX_MESSAGE: &str = "Syntax is not supported";

const RANGE_COMPONENT: &str = r"\s*(-?\d+(?:\.\d+)?|\w+(?:\.\w+)*)\s*";
const QUOTED_STRING: &str = r#"("[^"]*"|'[^']*')"#;
const REGULAR_TOKEN: &str = r"[^\s,]+";

fn range_pattern() -> String {
    format!(r"\(\s*{RANGE_COMPONENT}(\.{{2,}})\s*{RANGE_COMPONENT}\s*\)")
}

fn value_pattern() -> String {
    format!("({QUOTED_STRING}|{}|{REGULAR_TOKEN})", range_pattern())
}

fn key_value_pattern() -> String {
    format!(r"(\S+):\s*{}", value_pattern())
}

regex!(range_re, "", range_pattern());
regex!(value_re, "", value_pattern());
regex!(key_value_re, "", key_value_pattern());
regex!(
    fragments_re,
    "",
    format!("{QUOTED_STRING}|{}|{}|{REGULAR_TOKEN}", range_pattern(), key_value_pattern())
);

/// `getValuesInMarkup(markup).at(0)?.value`.
fn first_value(markup: &str) -> Option<&str> {
    value_re().exec_at(markup, 0).map(|m| m.as_str(markup))
}

/// `getValuesInMarkup`.
fn values_in_markup(markup: &str) -> Vec<&str> {
    value_re().find_iter(markup).iter().map(|m| m.as_str(markup)).collect()
}

struct Fragment<'s> {
    value: &'s str,
    index: usize,
}

/// `getFragmentsInMarkup`.
fn fragments_in_markup(markup: &str) -> Vec<Fragment<'_>> {
    fragments_re()
        .find_iter(markup)
        .iter()
        .map(|m| Fragment { value: m.as_str(markup), index: m.start() })
        .collect()
}

/// `doesFragmentContainUnsupportedParentheses`.
fn has_unsupported_parentheses(fragment: &str) -> bool {
    fragment.contains(['(', ')']) && !range_re().is_match(fragment)
}

/// `fragmentKeyValuePair(fragment)?.key`.
fn fragment_key(fragment: &str) -> Option<&str> {
    if !fragment.contains(':') {
        return None;
    }
    key_value_re().exec_at(fragment, 0).and_then(|m| m.group(fragment, 1))
}

/// `source.indexOf(markup, start)`. Markup strings are slices of the source that
/// start after the node does, so unlike upstream's `-1` there's nothing to handle:
/// `None` never happens in practice and the caller stops.
fn index_of(source: &str, markup: &str, start: u32) -> Option<u32> {
    let from = start as usize;
    source.get(from..)?.find(markup).map(|i| (from + i) as u32)
}

// `checks/InvalidBooleanExpressions.ts`.

fn detect_invalid_boolean_expression<'a>(node: &'a Node<'a>, cx: &LiquidCx<'a, '_>) {
    let Node::BooleanExpression(b) = node else { return };
    let left = match &b.condition {
        Node::Comparison(c) => &c.left,
        Node::LogicalExpression(c) => &c.left,
        _ => return,
    };
    let p = node.position();
    let left = left.position();
    let text = slice(cx.source(), left.start, left.end);
    cx.report(LiquidProblem::new(INVALID_SYNTAX_MESSAGE, p.start, p.end).fix(|c| c.replace(p.start, p.end, text)));
}

// `checks/MultipleAssignValues.ts`.

regex!(assign_re, "m", r"([^=]+)(=\s*)([^|]+)(?:\s*\|\s*.*)?$");

fn detect_multiple_assign_values<'a>(tag: &LiquidTag<'a>, cx: &LiquidCx<'a, '_>) {
    if tag.name != "assign" {
        return;
    }
    let Markup::Str(markup) = tag.markup else { return };
    let Some(m) = assign_re().exec_at(markup, 0) else { return };
    let (Some(variable), Some(operator), Some(value)) = (m.group(markup, 1), m.group(markup, 2), m.group(markup, 3))
    else {
        return;
    };
    let Some(first) = first_value(value) else { return };
    let Some(offset) = index_of(cx.source(), markup, tag.position.start) else { return };
    let base = offset + (variable.len() + operator.len()) as u32;
    let (start, end) = (base + first.len() as u32, base + trim_end(value).len() as u32);
    if end <= start {
        return;
    }
    cx.report(LiquidProblem::new(INVALID_SYNTAX_MESSAGE, start, end).fix(|c| c.replace(start, end, "")));
}

// `checks/InvalidEchoValue.ts`.

regex!(echo_re, "m", r"([^|]*)(?:\s*\|\s*.*)?$");

fn detect_invalid_echo_value<'a>(markup: &Markup<'a>, node_start: u32, cx: &LiquidCx<'a, '_>) {
    let Markup::Str(markup) = *markup else { return };
    if markup.is_empty() {
        return;
    }
    let Some(m) = echo_re().exec_at(markup, 0) else { return };
    let Some(echo_value) = m.group(markup, 1) else { return };
    let Some(offset) = index_of(cx.source(), markup, node_start) else { return };
    let Some(first) = first_value(echo_value) else {
        let (start, end) = (offset, offset + markup.len() as u32);
        cx.report(LiquidProblem::new(INVALID_SYNTAX_MESSAGE, start, end).fix(|c| c.replace(start, end, "blank")));
        return;
    };
    let (start, end) = (offset + first.len() as u32, offset + trim_end(echo_value).len() as u32);
    if end <= start {
        return;
    }
    cx.report(LiquidProblem::new(INVALID_SYNTAX_MESSAGE, start, end).fix(|c| c.replace(start, end, "")));
}

// `checks/InvalidLoopRange.ts`.

const INVALID_LOOP_RANGE_MESSAGE: &str =
    "Ranges must be in the format `(<start>..<end>)`. The start and end of the range must be whole numbers or variables.";

fn detect_invalid_loop_range<'a>(tag: &LiquidTag<'a>, node_start: u32, cx: &LiquidCx<'a, '_>) {
    if !is_loop_tag(tag.name) {
        return;
    }
    match &tag.markup {
        Markup::Str(markup) if !markup.is_empty() => validate_range_markup(markup, node_start, cx),
        Markup::Node(n) => {
            if let Node::ForMarkup(f) = &**n {
                if let Node::Range(_) = f.collection {
                    let p = f.collection.position();
                    validate_range_markup(slice(cx.source(), p.start, p.end), node_start, cx);
                }
            }
        }
        _ => {}
    }
}

fn validate_range_markup<'a>(markup: &str, node_start: u32, cx: &LiquidCx<'a, '_>) {
    let Some(m) = range_re().exec_at(markup, 0) else { return };
    let (Some(start), Some(end)) = (m.group(markup, 1), m.group(markup, 3)) else { return };
    let expected = format!("({}..{})", clean_range_component(start), clean_range_component(end));
    let full = m.as_str(markup);
    if full == expected {
        return;
    }
    let Some(markup_index) = index_of(cx.source(), markup, node_start) else { return };
    let from = markup_index + m.start() as u32;
    let to = from + full.len() as u32;
    cx.report(LiquidProblem::new(INVALID_LOOP_RANGE_MESSAGE, from, to).fix(|c| c.replace(from, to, expected)));
}

/// Truncates a component that is a number with a fraction, leaving variables alone.
fn clean_range_component(component: &str) -> String {
    let n = js_number(component);
    // `!isNaN(n) && n % 1 !== 0`; `Infinity % 1` is `NaN`, which isn't `0`.
    if !n.is_nan() && n % 1.0 != 0.0 {
        lq_json::js::number_to_string(n.trunc())
    } else {
        component.to_string()
    }
}

/// `Number(s)` for what a range component can hold: `-?\d+(\.\d+)?` or `\w+(\.\w+)*`.
fn js_number(s: &str) -> f64 {
    if s == "Infinity" {
        return f64::INFINITY;
    }
    if let Some(radix) = match s.get(..2) {
        Some("0x" | "0X") => Some(16),
        Some("0o" | "0O") => Some(8),
        Some("0b" | "0B") => Some(2),
        _ => None,
    } {
        let digits = &s[2..];
        if digits.is_empty() {
            return f64::NAN;
        }
        // Accumulating is exact enough: only "fractional or infinite" matters here.
        return digits.chars().try_fold(0f64, |n, c| c.to_digit(radix).map(|d| n * f64::from(radix) + f64::from(d))).unwrap_or(f64::NAN);
    }
    // Rust's parser also takes `inf`, `nan` and `infinity`, which JS spells differently.
    if s.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'-')) {
        s.parse().unwrap_or(f64::NAN)
    } else {
        f64::NAN
    }
}

// `checks/InvalidLoopArguments.ts`.

fn detect_invalid_loop_arguments<'a>(tag: &LiquidTag<'a>, node_start: u32, cx: &LiquidCx<'a, '_>) {
    if !is_loop_tag(tag.name) {
        return;
    }
    let source = cx.source();
    let markup: &str = match &tag.markup {
        Markup::Str(s) => s,
        Markup::Node(n) => {
            let p = n.position();
            slice(source, p.start, p.end)
        }
        _ => return,
    };
    if markup.is_empty() {
        return;
    }

    // The first three are `<variable> in <array>`: they may not have typed the rest yet.
    // Without quotes, parentheses or colons the fragments are just the plain tokens,
    // which covers most loops without running the regex.
    if !markup.contains(['"', '\'', '(', ':'])
        && markup.split(|c| lq_syntax::js::is_ws_char(c) || c == ',').filter(|t| !t.is_empty()).nth(3).is_none()
    {
        return;
    }
    let fragments = fragments_in_markup(markup);
    if fragments.len() <= 3 {
        return;
    }

    // Positional arguments are ignored after invalid positional or named ones.
    let mut no_more_positional = false;
    let mut filtered: Vec<&str> = Vec::new();
    let mut invalid: Vec<&str> = Vec::new();
    let tag_entry = cx.docset().and_then(|d| d.tag(tag.name));
    let is_supported = |key: &str, positional: bool| {
        tag_entry.is_some_and(|t| t.parameters.iter().any(|p| p.name == key && p.positional == positional))
    };

    for fragment in &fragments[3..] {
        if has_unsupported_parentheses(fragment.value) {
            no_more_positional = true;
            invalid.push(fragment.value);
            continue;
        }
        if let Some(key) = fragment_key(fragment.value) {
            no_more_positional = true;
            if !is_supported(key, false) {
                invalid.push(fragment.value);
                continue;
            }
        } else if no_more_positional || !is_supported(fragment.value, true) {
            no_more_positional = true;
            invalid.push(fragment.value);
            continue;
        }
        filtered.push(fragment.value);
    }
    if invalid.is_empty() {
        return;
    }

    let Some(markup_index) = index_of(source, markup, node_start) else { return };
    let last = &fragments[fragments.len() - 1];
    let start = markup_index + fragments[3].index as u32;
    let end = markup_index + (last.index + last.value.len()) as u32;
    let message = format!(
        "Arguments must be provided in the format `{} in <array> <positional arguments> <named arguments>`. Invalid/Unknown arguments: {}",
        tag.name,
        invalid.join(", "),
    );
    cx.report(LiquidProblem::new(message, start, end).fix(|c| c.replace(start, end, filtered.join(" "))));
}

// `checks/InvalidConditionalNodeParenthesis.ts`.

/// Whether it reported, as `detect...() || detectInvalidConditionalNode()`.
fn detect_unsupported_parenthesis<'a>(tag: &LiquidTag<'a>, node_start: u32, cx: &LiquidCx<'a, '_>) -> bool {
    if !matches!(tag.name, "if" | "elsif" | "unless") {
        return false;
    }
    let Markup::Str(markup) = tag.markup else { return false };
    if trim(markup).is_empty() || !markup.contains(['(', ')']) {
        return false;
    }
    let fragments = fragments_in_markup(markup);
    let bad: Vec<&Fragment<'_>> = fragments.iter().filter(|f| has_unsupported_parentheses(f.value)).collect();
    if bad.is_empty() {
        return false;
    }
    let Some(index) = index_of(cx.source(), markup, node_start) else { return false };
    let end = index + markup.len() as u32;
    cx.report(LiquidProblem::new(INVALID_SYNTAX_MESSAGE, index, end).fix(|c| {
        for fragment in &bad {
            let from = index + fragment.index as u32;
            let text: String = fragment.value.chars().filter(|c| !matches!(c, '(' | ')')).collect();
            c.replace(from, from + fragment.value.len() as u32, text);
        }
    }));
    true
}

// `checks/InvalidConditionalNode.ts`.

#[derive(Clone, Copy, PartialEq, Eq)]
enum TokenType {
    Logical,
    Comparison,
    Invalid,
    Literal,
    Variable,
}

struct Token<'s> {
    value: &'s str,
    ty: TokenType,
}

impl Token<'_> {
    fn is_value(&self) -> bool {
        matches!(self.ty, TokenType::Literal | TokenType::Variable)
    }
    fn is_operator(&self) -> bool {
        matches!(self.ty, TokenType::Logical | TokenType::Comparison)
    }
}

fn classify_token(value: &str, fixed: bool) -> TokenType {
    match value {
        "and" | "or" => TokenType::Logical,
        "==" | "!=" | ">=" | "<=" | ">" | "<" | "contains" => TokenType::Comparison,
        "@" | "#" | "$" | "&" => TokenType::Invalid,
        "true" | "false" | "nil" | "empty" | "blank" => TokenType::Literal,
        _ if is_quoted(value, fixed) || is_decimal(value) => TokenType::Literal,
        _ => TokenType::Variable,
    }
}

/// `['"][^'"]*['"]`. With `fixed` ([`Divergence::HtmlSyntaxQuotedLiteral`]) it is
/// `"[^"]*"|'[^']*'`, so `"it's"` is a literal, and `'a"` isn't.
fn is_quoted(value: &str, fixed: bool) -> bool {
    let b = value.as_bytes();
    if fixed {
        return b.len() >= 2 && matches!(b[0], b'\'' | b'"') && b[b.len() - 1] == b[0] && !b[1..b.len() - 1].contains(&b[0]);
    }
    b.len() >= 2
        && matches!(b[0], b'\'' | b'"')
        && matches!(b[b.len() - 1], b'\'' | b'"')
        && !b[1..b.len() - 1].iter().any(|c| matches!(c, b'\'' | b'"'))
}

/// `\d+(?:\.\d+)?`.
fn is_decimal(value: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match value.split_once('.') {
        Some((whole, fraction)) => digits(whole) && digits(fraction),
        None => digits(value),
    }
}

struct ExpressionIssue {
    message: String,
    fix: String,
}

fn detect_invalid_conditional_node<'a>(
    name: Option<&str>,
    markup: &Markup<'a>,
    opening: lq_syntax::Position,
    cx: &LiquidCx<'a, '_>,
) {
    if !matches!(name, Some("if" | "elsif" | "unless")) {
        return;
    }
    let Markup::Str(markup) = *markup else { return };
    let Some(issue) = analyze_conditional_expression(markup, cx.divergence(Divergence::HtmlSyntaxQuotedLiteral)) else { return };
    let opening_tag = slice(cx.source(), opening.start, opening.end);
    let Some(offset) = opening_tag.find(markup) else { return };
    let start = opening.start + offset as u32;
    let end = start + markup.len() as u32;
    cx.report(
        LiquidProblem::new(format!("{INVALID_SYNTAX_MESSAGE}: {}", issue.message), start, end)
            .fix(|c| c.replace(start, end, issue.fix)),
    );
}

fn analyze_conditional_expression(markup: &str, fixed: bool) -> Option<ExpressionIssue> {
    let trimmed = trim(markup);
    if trimmed.is_empty() {
        return None;
    }
    let tokens: Vec<Token<'_>> = values_in_markup(trimmed)
        .into_iter()
        .map(|value| Token { value, ty: classify_token(value, fixed) })
        .collect();
    if tokens.is_empty() {
        return None;
    }
    check_invalid_starting_token(&tokens)
        .or_else(|| check_trailing_tokens_after_comparison(&tokens))
        .or_else(|| check_lax_parsing_issues(&tokens))
}

fn check_invalid_starting_token(tokens: &[Token<'_>]) -> Option<ExpressionIssue> {
    let first = &tokens[0];
    // Alone, `contains` is a variable of that name, not the operator.
    if tokens.len() == 1 && first.value == "contains" {
        return None;
    }
    if matches!(first.ty, TokenType::Invalid | TokenType::Comparison) {
        return Some(ExpressionIssue {
            message: format!("Conditional cannot start with '{}'. Use a variable or value instead", first.value),
            fix: "false".to_string(),
        });
    }
    None
}

fn join_values(tokens: &[Token<'_>]) -> String {
    tokens.iter().map(|t| t.value).collect::<Vec<_>>().join(" ")
}

fn check_trailing_tokens_after_comparison(tokens: &[Token<'_>]) -> Option<ExpressionIssue> {
    if tokens.len() < 4 {
        return None;
    }
    for i in 0..=tokens.len() - 4 {
        let (v1, op, v2) = (&tokens[i], &tokens[i + 1], &tokens[i + 2]);
        let remaining = &tokens[i + 3..];
        if v1.is_value() && op.ty == TokenType::Comparison && v2.is_value() && remaining[0].ty != TokenType::Logical {
            let valid = join_values(&tokens[..i + 3]);
            let junk = join_values(remaining);
            let advice = if junk.contains("&&") || junk.contains("||") {
                ". Use 'and'/'or' instead of '&&'/'||' for multiple conditions"
            } else {
                ""
            };
            return Some(ExpressionIssue {
                message: format!("Conditional is invalid. Anything after '{valid}' will be ignored{advice}"),
                fix: valid,
            });
        }
    }
    None
}

fn check_lax_parsing_issues(tokens: &[Token<'_>]) -> Option<ExpressionIssue> {
    for i in 0..tokens.len().saturating_sub(1) {
        let current = &tokens[i];
        let next = &tokens[i + 1];
        if current.ty == TokenType::Literal && !next.is_operator() {
            let remaining = &tokens[i + 1..];
            let unknown_operator = remaining[0].ty == TokenType::Variable && remaining.iter().any(Token::is_operator);
            if !unknown_operator {
                let ignored = join_values(remaining);
                let advice = if ignored.contains("&&") || ignored.contains("||") {
                    ". Use 'and'/'or' instead of '&&'/'||' for multiple conditions"
                } else {
                    ""
                };
                return Some(ExpressionIssue {
                    message: format!(
                        "Expression stops at truthy value '{}', and will ignore: '{ignored}'{advice}",
                        current.value
                    ),
                    fix: current.value.to_string(),
                });
            }
        }
    }
    None
}

// `checks/InvalidFilterName.ts`.

/// The tokenizer drops an invalid byte after a filter name, so the markup parses and
/// only the source still shows it.
fn detect_invalid_filter_name_in_filters<'a>(variable: &LiquidVariable<'a>, markup_end: u32, cx: &LiquidCx<'a, '_>) {
    let source = cx.source();
    for filter in &variable.filters {
        let Node::LiquidFilter(f) = filter else { continue };
        let Some(name_start) = index_of(source, f.name, filter.position().start) else { continue };
        let trailing_start = name_start + f.name.len() as u32;
        let rest = slice(source, trailing_start, markup_end);
        // `/^([^\s:|]+)/`. Most filters stop here, before the known-filter lookup.
        let len = rest
            .char_indices()
            .find(|&(_, c)| lq_syntax::js::is_ws_char(c) || matches!(c, ':' | '|'))
            .map_or(rest.len(), |(i, _)| i);
        if len == 0 {
            continue;
        }
        if !is_known_filter(f.name, cx) {
            continue;
        }
        let trailing_end = trailing_start + len as u32;
        report_trailing_characters(f.name, &rest[..len], trailing_start, trailing_end, cx);
    }
}

fn is_known_filter(name: &str, cx: &LiquidCx<'_, '_>) -> bool {
    cx.docset().is_some_and(|d| d.filter(name).is_some())
}

fn report_trailing_characters(name: &str, trailing: &str, start: u32, end: u32, cx: &LiquidCx<'_, '_>) {
    cx.report(
        LiquidProblem::new(
            format!("{INVALID_SYNTAX_MESSAGE} Filter '{name}' has trailing characters '{trailing}' that should be removed."),
            start,
            end,
        )
        .fix(|c| c.replace(start, end, "")),
    );
}

regex!(filter_name_re, "", r"\|\s*([a-zA-Z][a-zA-Z0-9_]*)");
regex!(
    invalid_segment_re,
    "",
    r"^(?!\s*(?::|$|\|\s*[a-zA-Z]|\|\s*\||\s*\|\s*(?:[}%]|$)))([^:|]+?)(?=\s*(?::|$|\|))"
);

fn detect_invalid_filter_name_in_markup<'a>(markup: &str, node_start: u32, cx: &LiquidCx<'a, '_>) {
    if !markup.contains('|') || cx.docset().is_none() {
        return;
    }
    let trimmed = trim(markup);
    for m in filter_name_re().find_iter(trimmed) {
        let Some(name) = m.group(trimmed, 1) else { continue };
        if !is_known_filter(name, cx) {
            continue;
        }
        let after = &trimmed[m.end()..];
        let Some(segment) = invalid_segment_re().exec_at(after, 0) else { continue };
        let Some(segment) = segment.group(after, 1) else { continue };
        let Some(markup_start) = index_of(cx.source(), markup, node_start) else { continue };
        let start = markup_start + m.end() as u32;
        report_trailing_characters(name, segment, start, start + segment.len() as u32, cx);
    }
}

// `checks/InvalidPipeSyntax.ts`.

regex!(extra_pipes_re, "", r"(\|\s*){2,}");
regex!(trailing_pipe_re, "", r"(\s*\|)+\s*$");

fn detect_invalid_pipe_syntax<'a>(markup: &str, node_start: u32, cx: &LiquidCx<'a, '_>) {
    if !markup.contains('|') {
        return;
    }
    let Some(markup_start) = index_of(cx.source(), markup, node_start) else { return };
    let trimmed = trim(markup);
    for m in extra_pipes_re().find_iter(trimmed) {
        // Pipes that end the markup are a trailing pipe, reported below.
        if m.end() == trimmed.len() {
            continue;
        }
        let (start, end) = (markup_start + m.start() as u32, markup_start + m.end() as u32);
        cx.report(
            LiquidProblem::new(format!("{INVALID_SYNTAX_MESSAGE}. Remove extra `|` character(s)."), start, end)
                .fix(|c| c.replace(start, end, "| ")),
        );
    }
    if let Some(m) = trailing_pipe_re().exec_at(trimmed, 0) {
        let (start, end) = (markup_start + m.start() as u32, markup_start + m.end() as u32);
        cx.report(
            LiquidProblem::new(format!("{INVALID_SYNTAX_MESSAGE}. Remove the trailing `|` character."), start, end)
                .fix(|c| c.replace(start, end, "")),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `checks/utils.spec.ts`, which doesn't go through `check()`.

    #[test]
    fn values_in_markup_spec() {
        assert_eq!(values_in_markup(r#""123""#), [r#""123""#]);
        let m = value_re().find_iter(r#""123" 555 text"#);
        let found: Vec<_> = m.iter().map(|m| (m.as_str(r#""123" 555 text"#), m.start())).collect();
        assert_eq!(found, [(r#""123""#, 0), ("555", 6), ("text", 10)]);
        assert!(values_in_markup("").is_empty());
        assert!(values_in_markup("     ").is_empty());
    }

    #[test]
    fn fragments_in_markup_spec() {
        for sep in [" ", ", "] {
            let markup = format!("reversed{sep}limit: 10{sep}offset: 1{sep}something");
            let values: Vec<_> = fragments_in_markup(&markup).iter().map(|f| f.value.to_string()).collect();
            assert_eq!(values, ["reversed", "limit: 10", "offset: 1", "something"]);
        }
        let markup = "(1..10) ( 1..10 ) ( 1 .. 10 ) ( 1 ... 10 )";
        let values: Vec<_> = fragments_in_markup(markup).iter().map(|f| f.value).collect();
        assert_eq!(values, ["(1..10)", "( 1..10 )", "( 1 .. 10 )", "( 1 ... 10 )"]);
    }

    #[test]
    fn fragment_key_value_pair_spec() {
        assert_eq!(fragment_key("key: value"), Some("key"));
        assert_eq!(fragment_key("key"), None);
    }

    #[test]
    fn unsupported_parentheses_spec() {
        assert!(has_unsupported_parentheses("(this and that)"));
        assert!(!has_unsupported_parentheses("(1..2)"));
        assert!(!has_unsupported_parentheses("plain"));
    }

    // What the specs leave to JS itself.

    #[test]
    fn number_semantics() {
        for (s, want) in [
            ("1.5", "1"),
            ("-1.5", "-1"),
            ("-0.5", "0"),
            ("2", "2"),
            ("x", "x"),
            ("a.b", "a.b"),
            ("1e3", "1e3"),
            ("1.5e1", "1.5e1"),
            ("1.25e1", "12"),
            ("0x10", "0x10"),
            ("0xZ", "0xZ"),
            ("1e400", "Infinity"),
            ("Infinity", "Infinity"),
            ("inf", "inf"),
            ("nan", "nan"),
            ("1_000", "1_000"),
        ] {
            assert_eq!(clean_range_component(s), want, "{s}");
        }
    }

    #[test]
    fn substitution_patterns() {
        assert_eq!(get_substitution("a$$b", "M", "B", "A"), "a$b");
        assert_eq!(get_substitution("$&", "M", "B", "A"), "M");
        assert_eq!(get_substitution("$`|$'", "M", "B", "A"), "B|A");
        assert_eq!(get_substitution("$1 $x $", "M", "B", "A"), "$1 $x $");
    }

    #[test]
    fn clean_error_message_spec() {
        assert_eq!(clean_error_message("Line 1, col 3:  expected x not y", "H"), "SyntaxError: expected x not \"H\"");
        assert_eq!(clean_error_message("a not b\nc", "$&"), "a not \" not b\"\nc");
        assert_eq!(clean_error_message("nothing here", "H"), "nothing here");
    }

    #[test]
    fn line_col_counts_utf16_units() {
        let source = "ab\n\u{1F600}\u{e9}c";
        assert_eq!(line_col_to_byte(source, 1, 1), 0);
        assert_eq!(line_col_to_byte(source, 2, 1), 3);
        assert_eq!(line_col_to_byte(source, 2, 3), 7);
        assert_eq!(line_col_to_byte(source, 2, 4), 9);
        // The second unit of the emoji.
        assert_eq!(line_col_to_byte(source, 2, 2), 5);
        assert_eq!(next_utf16_unit(source, 5), 7);
        assert_eq!(next_utf16_unit(source, 3), 5);
        assert_eq!(next_utf16_unit(source, 7), 9);
        assert_eq!(next_utf16_unit(source, 10), 11);
        assert_eq!(lossy_slice(source, 5, 7), "\u{FFFD}");
        assert_eq!(lossy_slice(source, 3, 5), "\u{FFFD}");
        assert_eq!(lossy_slice(source, 3, 7), "\u{1F600}");
        assert_eq!(lossy_slice(source, 7, 9), "\u{e9}");
    }

    #[test]
    fn token_classes() {
        for (value, want) in [
            ("and", TokenType::Logical),
            ("contains", TokenType::Comparison),
            ("$", TokenType::Invalid),
            ("'a'", TokenType::Literal),
            ("'a\"", TokenType::Literal),
            ("'a'b'", TokenType::Variable),
            ("'", TokenType::Variable),
            ("1.5", TokenType::Literal),
            ("1.", TokenType::Variable),
            ("1.5.2", TokenType::Variable),
            ("nil", TokenType::Literal),
            ("x", TokenType::Variable),
        ] {
            assert!(classify_token(value, false) == want, "{value}");
        }
    }
}
