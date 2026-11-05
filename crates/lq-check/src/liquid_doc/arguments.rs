//! `liquid-doc/arguments.ts`: reports shared by the `render` and `content_for` checks.
//! `node` is a `ContentForMarkup` or `RenderMarkup`; `arg`s are `NamedArgument`s.

use super::{argument_type_mismatch_message, check_argument_type, default_value_for_type, parse_string_literals, ArgumentTypeCheck, LiquidDocParameter};
use crate::api::{Divergence, LiquidCx, LiquidProblem};
use crate::fixes::StringCorrector;
use lq_json::js::is_js_whitespace;
use lq_syntax::Node;

fn owner_message(node: &Node<'_>, name: &str) -> String {
    match node {
        Node::ContentForMarkup(_) => format!(" in content_for tag for static block '{name}'"),
        Node::RenderMarkup(_) => format!(" in render tag for snippet '{name}'"),
        _ => String::new(),
    }
}

fn args_of<'n, 's>(node: &'n Node<'s>) -> &'n [Node<'s>] {
    match node {
        Node::ContentForMarkup(m) => &m.args,
        Node::RenderMarkup(m) => &m.args,
        _ => &[],
    }
}

fn arg_name<'n>(arg: &'n Node<'_>) -> &'n str {
    match arg {
        Node::NamedArgument(a) => &a.name,
        _ => "",
    }
}

/// `Map<string, LiquidDocParameter>` in insertion order: a repeated name takes the
/// later definition and keeps the first slot.
pub struct DocParams<'a>(Vec<&'a LiquidDocParameter>);

impl<'a> DocParams<'a> {
    pub fn get(&self, name: &str) -> Option<&'a LiquidDocParameter> {
        self.0.iter().find(|p| p.name == name).copied()
    }

    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn values(&self) -> impl Iterator<Item = &'a LiquidDocParameter> + '_ {
        self.0.iter().copied()
    }
}

/// The file holding the `@param`s of `node`'s target.
fn declaring_file(node: &Node<'_>, name: &str) -> String {
    match node {
        Node::ContentForMarkup(_) => format!("blocks/{name}.liquid"),
        _ => format!("snippets/{name}.liquid"),
    }
}

/// How many `@param`s a hint lists by name.
const LISTED_PARAMS: usize = 6;

fn declared_note(node: &Node<'_>, name: &str, params: &DocParams<'_>) -> String {
    let file = declaring_file(node, name);
    let names: Vec<&str> = params.values().map(|p| p.name.as_str()).collect();
    if names.is_empty() {
        return format!("{file} declares no arguments");
    }
    let listed = names[..names.len().min(LISTED_PARAMS)].join(", ");
    match names.len().saturating_sub(LISTED_PARAMS) {
        0 => format!("{file} declares {listed}"),
        more => format!("{file} declares {listed}, +{more} more"),
    }
}

fn required_note(node: &Node<'_>, name: &str, param: &LiquidDocParameter) -> String {
    let file = declaring_file(node, name);
    let ty = param.ty.as_deref().map_or(String::new(), |t| format!(" {{{t}}}"));
    let description = param.description.as_deref().map_or(String::new(), |d| format!(": {d}"));
    format!("{file} declares @param{ty} {}{description}", param.name)
}

/// `reportUnknownArguments`.
pub fn report_unknown_arguments<'a>(cx: &LiquidCx<'a, '_>, node: &'a Node<'a>, args: &[&'a Node<'a>], name: &str, params: &DocParams<'_>) {
    let owner = owner_message(node, name);
    for &arg in args {
        let a = arg_name(arg);
        let p = arg.position();
        cx.report(
            LiquidProblem::new(format!("Unknown argument '{a}'{owner}."), p.start, p.end)
                .hints_if(cx.wants_hints(), |problem| {
                    problem
                        .label("unknown argument")
                        .note_opt(crate::check_utils::closest(a, params.values().map(|p| p.name.as_str())).map(crate::check_utils::did_you_mean))
                        .note(declared_note(node, name, params))
                })
                .suggest(format!("Remove '{a}'"), |c| remove_argument(c, cx.source(), node, arg)),
        );
    }
}

/// `reportMissingArguments`.
pub fn report_missing_arguments<'a>(cx: &LiquidCx<'a, '_>, node: &'a Node<'a>, missing: &[&LiquidDocParameter], name: &str) {
    let owner = owner_message(node, name);
    let p = node.position();
    for param in missing {
        let n = &param.name;
        cx.report(
            LiquidProblem::new(format!("Missing required argument '{n}'{owner}."), p.start, p.end)
                .hints_if(cx.wants_hints(), |problem| problem.note(required_note(node, name, param)))
                .suggest(format!("Add required argument '{n}'"), |c| {
                    add_argument(c, cx.source(), node, param, cx.divergence(Divergence::AddArgumentSuggestion))
                }),
        );
    }
}

/// `reportDuplicateArguments`.
pub fn report_duplicate_arguments<'a>(cx: &LiquidCx<'a, '_>, node: &'a Node<'a>, duplicates: &[&'a Node<'a>], name: &str) {
    let owner = owner_message(node, name);
    for &arg in duplicates {
        let a = arg_name(arg);
        let p = arg.position();
        cx.report(
            LiquidProblem::new(format!("Duplicate argument '{a}'{owner}."), p.start, p.end)
                .suggest(format!("Remove duplicate argument '{a}'"), |c| remove_argument(c, cx.source(), node, arg)),
        );
    }
}

/// `findTypeMismatchParams`.
pub fn find_type_mismatch_params<'a>(params: &DocParams<'_>, provided: &'a [Node<'a>]) -> Vec<&'a Node<'a>> {
    provided
        .iter()
        .filter(|arg| {
            let Node::NamedArgument(a) = arg else { return false };
            let def = params.get(&a.name);
            match def.and_then(|d| d.ty.as_deref()) {
                Some(t) if !t.is_empty() => check_argument_type(t, &a.value) == ArgumentTypeCheck::Incompatible,
                _ => false,
            }
        })
        .collect()
}

/// `reportTypeMismatches`.
pub fn report_type_mismatches<'a>(cx: &LiquidCx<'a, '_>, args: &[&'a Node<'a>], params: &DocParams<'_>) {
    for arg in args {
        let Node::NamedArgument(a) = arg else { continue };
        let Some(ty) = params.get(&a.name).and_then(|d| d.ty.as_deref()).filter(|t| !t.is_empty()) else {
            continue;
        };
        let p = a.value.position();
        let problem = LiquidProblem::new(argument_type_mismatch_message(&a.name, ty, &a.value, cx.source()), p.start, p.end);
        cx.report(with_type_mismatch_suggestions(problem, ty, p.start, p.end));
    }
}

/// `generateTypeMismatchSuggestions`, added onto a problem.
pub fn with_type_mismatch_suggestions(
    mut problem: LiquidProblem,
    expected: &str,
    start: u32,
    end: u32,
) -> LiquidProblem {
    if let Some(literals) = parse_string_literals(expected) {
        for literal in literals {
            problem = problem.suggest(format!("Replace with {}", literal.raw), |c| c.replace(start, end, literal.raw.clone()));
        }
        return problem;
    }
    let expected = expected.to_lowercase();
    let default = default_value_for_type(Some(&expected));
    if !default.is_empty() {
        problem = problem.suggest(format!("Replace with default value '{default}' for {expected}"), |c| {
            c.replace(start, end, default.clone());
        });
    }
    problem.suggest("Remove value", |c| c.remove(start, end))
}

fn is_last_arg(node: &Node<'_>, arg: &Node<'_>) -> bool {
    let args = args_of(node);
    args.len() == 1 || args.last().is_some_and(|l| l.position().start == arg.position().start)
}

/// `getBlockName(node)`.
pub fn block_name<'n>(node: &'n Node<'_>) -> Option<&'n str> {
    let Node::ContentForMarkup(m) = node else { return None };
    let Node::String(ty) = &m.content_for_type else { return None };
    if ty.value != "block" {
        return None;
    }
    let arg = m.args.iter().find(|a| arg_name(a) == "type")?;
    let Node::NamedArgument(a) = arg else { return None };
    let Node::String(s) = &a.value else { return None };
    Some(s.value)
}

/// `getSnippetName(node)`.
pub fn snippet_name<'n>(node: &'n Node<'_>) -> Option<&'n str> {
    let Node::RenderMarkup(m) = node else { return None };
    let Node::String(s) = &m.snippet else { return None };
    Some(s.value)
}

/// `getLiquidDocParams(context, relativePath)`.
pub fn liquid_doc_params<'a, C>(cx: &crate::api::Cx<'a, '_, C>, relative_path: &str) -> Option<DocParams<'a>> {
    let doc = cx.doc_definition(relative_path)?;
    let params = doc.liquid_doc.as_ref()?.parameters.as_ref()?;
    let mut map: Vec<&'a LiquidDocParameter> = Vec::with_capacity(params.len());
    for p in params {
        match map.iter_mut().find(|slot| slot.name == p.name) {
            Some(slot) => *slot = p,
            None => map.push(p),
        }
    }
    Some(DocParams(map))
}

fn trailing_comma_len(s: &str) -> Option<usize> {
    // /^\s*,\s*/
    let ws = s.len() - s.trim_start_matches(is_js_whitespace).len();
    let rest = s[ws..].strip_prefix(',')?;
    let after = rest.len() - rest.trim_start_matches(is_js_whitespace).len();
    Some(ws + 1 + after)
}

/// `/,\s*$/`: the length of the match at the end.
fn leading_comma_len(s: &str) -> Option<usize> {
    let t = s.trim_end_matches(is_js_whitespace);
    t.ends_with(',').then(|| s.len() - t.len() + 1)
}

/// `makeRemoveArgumentCorrector(node, arg)` applied.
pub fn remove_argument(c: &mut StringCorrector, source: &str, node: &Node<'_>, arg: &Node<'_>) {
    let (np, ap) = (node.position(), arg.position());
    let before = lq_syntax::js::slice(source, np.start, ap.start);
    let after = lq_syntax::js::slice(source, ap.end, np.end);
    if let Some(trailing) = trailing_comma_len(after) {
        if is_last_arg(node, arg) {
            let start = leading_comma_len(before).map_or(ap.start, |l| ap.start - l as u32);
            let matched = &after[..trailing];
            let without_ws = matched.trim_end_matches(is_js_whitespace).len();
            c.remove(start, ap.end + without_ws as u32);
            return;
        }
        c.remove(ap.start, ap.end + trailing as u32);
        return;
    }
    let start = leading_comma_len(before).map_or(ap.start, |l| ap.start - l as u32);
    c.remove(start, ap.end);
}

/// `makeAddArgumentCorrector(node, arg)` applied.
///
/// With `fixed` ([`Divergence::AddArgumentSuggestion`](crate::api::Divergence)):
/// - With no arguments, the insertion goes after the markup rather than at `end - 1`, and
///   replaces a trailing comma. Upstream gives `{% render 'x',, a: 1 %}` after a comma, and
///   inserts inside the quotes for `{% render 'x'%}`.
/// - A type with no literal default (`object`, a drop, or no type at all) gets `name: name`.
///   Upstream's empty value, `name: `, is a syntax error, and no literal is valid: `nil` fails
///   `ValidRenderSnippetArgumentTypes`. A variable of the same name parses, type-checks,
///   is the common idiom, and `UndefinedObject` flags it if there's no such variable.
pub fn add_argument(c: &mut StringCorrector, source: &str, node: &Node<'_>, param: &LiquidDocParameter, fixed: bool) {
    let np = node.position();
    let default = default_value_for_type(param.ty.as_deref());
    let value = if fixed && default.is_empty() { param.name.as_str() } else { default.as_str() };
    let to_add = format!(", {}: {value}", param.name);
    let args = args_of(node);
    let Some(last) = args.last() else {
        if fixed {
            let markup = lq_syntax::js::slice(source, np.start, np.end);
            let trimmed = markup.trim_end_matches(is_js_whitespace);
            let end = np.start + trimmed.len() as u32;
            match trimmed.strip_suffix(',') {
                Some(head) => {
                    let start = np.start + head.trim_end_matches(is_js_whitespace).len() as u32;
                    c.replace(start, end, to_add);
                }
                None => c.insert(end, to_add),
            }
            return;
        }
        c.insert(np.end - 1, to_add);
        return;
    };
    let last_end = last.position().end;
    let after = lq_syntax::js::slice(source, last_end, np.end);
    // /\s*,\s*/ (unanchored): the length of the first match.
    if let Some(comma) = after.find(',') {
        let ws_before = after[..comma].len() - after[..comma].trim_end_matches(is_js_whitespace).len();
        let rest = &after[comma + 1..];
        let ws_after = rest.len() - rest.trim_start_matches(is_js_whitespace).len();
        let len = ws_before + 1 + ws_after;
        c.replace(last_end, last_end + len as u32, format!("{to_add} "));
        return;
    }
    c.insert(last_end, to_add);
}
