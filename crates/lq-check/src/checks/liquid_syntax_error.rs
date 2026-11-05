//! `LiquidSyntaxError`: `checks/liquid-syntax-error/`.

use super::shared_liquid_syntax::*;
use crate::api::*;
use lq_syntax::js::{slice, trim};
use lq_syntax::tags::{tag_for_name, TagDefinition};
use lq_syntax::{
    tokenize, LiquidBranch, LiquidRawTag, LiquidTag, LiquidVariableOutput, Markup, Node, NodeId,
    ParseError, Position, TokenType, TokenizeOptions,
};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[
        NodeKind::LiquidRawTag,
        NodeKind::LiquidTag,
        NodeKind::LiquidBranch,
        NodeKind::LiquidVariableOutput,
    ]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(LiquidSyntaxError::default()))
}

const BARE: &str = "Bare bracket access is not allowed";
const BARE_STRICT2: &str = "Bare bracket access is not allowed in strict2 mode";

const COMMENT_RAW_BODY_ERROR: &str = "Liquid syntax error: 'comment' tag was never closed";
const UNMATCHED_COMMENT_CLOSE: &str =
    "Attempting to close LiquidTag 'comment' before it was opened without a matching 'comment'";
const UNMATCHED_RAW_CLOSE: &str =
    "Attempting to close LiquidTag 'raw' before it was opened without a matching 'raw'";
const UNCLOSED_COMMENT: &str = "Attempting to end parsing before LiquidRawTag 'comment' was closed";
const UNCLOSED_BLOCK: &str = "Attempting to end parsing before LiquidTag 'block' was closed";
const UNCLOSED_BLOCK_IN_LIQUID: &str = "Unclosed block tag 'block' in {% liquid %} block";
const UNOPENED_BLOCK: &str =
    "Attempting to close LiquidTag 'block' before it was opened without a matching 'block'";
const UNCLOSED_DOC: &str = "Attempting to end parsing before LiquidRawTag 'doc' was closed";
const UNOPENED_DOC: &str =
    "Attempting to close LiquidTag 'doc' before it was opened without a matching 'doc'";
const UNCLOSED_PARTIAL: &str = "Attempting to end parsing before LiquidTag 'partial' was closed";
const UNCLOSED_PARTIAL_IN_LIQUID: &str = "Unclosed block tag 'partial' in {% liquid %} block";
const UNOPENED_PARTIAL: &str =
    "Attempting to close LiquidTag 'partial' before it was opened without a matching 'partial'";
const UNCLOSED_RAW: &str = "Attempting to end parsing before LiquidRawTag 'raw' was closed";
const UNCLOSED_RAW_IN_LIQUID: &str = "Unclosed raw tag 'raw' in {% liquid %} block";
const UNCLOSED_JAVASCRIPT: &str =
    "Attempting to end parsing before LiquidRawTag 'javascript' was closed";
const UNOPENED_JAVASCRIPT: &str =
    "Attempting to close LiquidTag 'javascript' before it was opened without a matching 'javascript'";
const JAVASCRIPT_SYNTAX_ERROR: &str = "Syntax Error in 'javascript' - Valid syntax: javascript";

#[derive(Default)]
struct LiquidSyntaxError {
    /// `reportedUnknownLiquidBlockTags`.
    reported: Vec<NodeId>,
}

fn report(cx: &LiquidCx<'_, '_>, p: Position, message: &str) {
    cx.report(LiquidProblem::new(message, p.start, p.end));
}

fn report_at(cx: &LiquidCx<'_, '_>, (start, end): (u32, u32), message: &str) {
    cx.report(LiquidProblem::new(message, start, end));
}

impl<'a> LiquidCheck<'a> for LiquidSyntaxError {
    fn on_code_path_start(&mut self, cx: &LiquidCx<'a, '_>) {
        let Err(error) = cx.file().liquid_ast() else { return };
        let source = cx.source();
        check_comment_parser_error(error, cx, source);
        check_block_parser_error(error, cx, source);
        check_doc_parser_error(error, cx, source);
        check_partial_parser_error(error, cx, source);
        check_raw_parser_error(error, cx, source);
        check_javascript_parser_error(error, cx, source);
    }

    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        match node {
            Node::LiquidRawTag(tag) => check_raw_tag_node(tag, cx),
            Node::LiquidTag(tag) => self.check_tag(tag, cx),
            Node::LiquidBranch(branch) => check_branch(branch, cx),
            Node::LiquidVariableOutput(output) => check_variable_output(output, cx),
            _ => {}
        }
    }
}

fn check_raw_tag_node(tag: &LiquidRawTag<'_>, cx: &LiquidCx<'_, '_>) {
    let raw = slice(cx.source(), tag.markup_position.start, tag.markup_position.end);
    match tag.name {
        "comment" => check_comment_raw_tag(tag, cx),
        "doc" => check_doc_tag(tag, cx),
        "javascript" => {
            if !has_empty_markup(raw) {
                report(cx, tag.block_start_position, JAVASCRIPT_SYNTAX_ERROR);
            }
        }
        "schema" => {
            if !tag.markup.is_empty() {
                report(cx, tag.markup_position, "Syntax error in 'schema' tag");
            }
        }
        "style" => {
            if !has_empty_markup(raw) {
                let message = format!("'style' tag does not accept any arguments in \"{raw}\"");
                report(cx, tag.block_start_position, &message);
            }
        }
        "stylesheet" => {
            if !has_empty_markup(raw) && !has_single_string_markup(raw, "scss") {
                let message =
                    format!("'stylesheet' tag can only accept the string argument 'scss' in \"{raw}\"");
                report(cx, tag.block_start_position, &message);
            }
        }
        "raw"
            if !tag.markup.is_empty()
                || has_skipped_prefix_characters(cx.source(), tag.markup_position.start, tag.markup_position.end)
                || has_skipped_characters(raw) =>
        {
            report(cx, tag.markup_position, "Syntax error in 'raw' tag");
        }
        _ => {}
    }
}

impl LiquidSyntaxError {
    fn check_tag(&mut self, tag: &LiquidTag<'_>, cx: &LiquidCx<'_, '_>) {
        let source = cx.source();
        let p = tag.position;
        let raw = slice(source, tag.markup_position.start, tag.markup_position.end);
        match tag.name {
            "#" => {
                if !has_ruby_valid_inline_comment_markup(raw) {
                    report(cx, p, "Syntax error in tag '#' - Each line of comments must be prefixed by the '#' character");
                }
            }
            "assign" => check_assign(tag, raw, cx),
            "block" => check_block(tag, raw, cx),
            "break" | "continue" | "ifchanged" => {}
            "capture" => check_simple_lookup(tag, raw, cx, "Syntax error in 'capture' tag"),
            "case" => check_case(tag, raw, cx),
            "content_for" => check_content_for(tag, cx),
            "cycle" => check_cycle(tag, raw, cx),
            "decrement" => check_simple_lookup(tag, raw, cx, "Syntax error in 'decrement' tag"),
            "echo" => check_echo(tag, raw, cx),
            "form" => check_form(tag, raw, cx),
            "for" => check_loop(tag, cx, "Syntax error in 'for' tag", false),
            "tablerow" => check_loop(tag, cx, "Syntax error in 'tablerow' tag", true),
            "if" => check_condition(tag, raw, cx, "Syntax error in 'if' tag"),
            "unless" => check_condition(tag, raw, cx, "Syntax error in 'unless' tag"),
            "include" => check_include(tag, raw, cx),
            "increment" => check_increment(tag, raw, cx),
            "layout" => check_layout(tag, raw, cx),
            "paginate" => check_paginate(tag, cx),
            "partial" => check_partial(tag, raw, cx),
            "render" => check_render(tag, raw, cx),
            "section" => check_section(tag, raw, cx),
            "sections" => {
                if matches!(tag.markup, Markup::Str(_)) || has_skipped_characters(raw) {
                    report(cx, p, "Syntax error in 'sections' tag");
                }
            }
            "else" | "elsif" | "when" => report(cx, p, &format!("Unknown tag '{}'", tag.name)),
            _ => self.check_base_tag(tag, cx),
        }
    }

    /// `checkBaseTag`.
    fn check_base_tag(&mut self, tag: &LiquidTag<'_>, cx: &LiquidCx<'_, '_>) {
        if let Some(reason) = &tag.reason {
            let location = (tag.name == "liquid" && cx.divergence(Divergence::LiquidSyntaxUnclosedLine))
                .then(|| unclosed_statement_of(tag, reason, cx))
                .flatten();
            match location {
                Some(location) => report_at(cx, location, reason),
                None => report(cx, tag.position, reason),
            }
            return;
        }
        if let ("liquid", Markup::List(statements)) = (tag.name, &tag.markup) {
            for statement in statements {
                if let Node::LiquidTag(statement) = statement {
                    self.report_unknown(statement, cx);
                }
            }
            return;
        }
        self.report_unknown(tag, cx);
    }

    fn report_unknown(&mut self, tag: &LiquidTag<'_>, cx: &LiquidCx<'_, '_>) {
        if !is_unknown_tag_inside_liquid_block(tag, cx) || self.reported.contains(&tag.id) {
            return;
        }
        self.reported.push(tag.id);
        report(cx, tag.position, &format!("Unknown tag '{}'", tag.name));
    }
}

/// Where an unclosed `block` or `partial` in this `{% liquid %}` tag really is.
fn unclosed_statement_of(tag: &LiquidTag<'_>, reason: &str, cx: &LiquidCx<'_, '_>) -> Option<(u32, u32)> {
    let name = match reason {
        UNCLOSED_BLOCK_IN_LIQUID => "block",
        UNCLOSED_PARTIAL_IN_LIQUID => "partial",
        _ => return None,
    };
    let source = slice(cx.source(), tag.position.start, tag.position.end);
    unclosed_liquid_statement(source, tag.position.start, name)
}

/// `isUnknownTagInsideLiquidBlock`, with `knownLiquidTagsFor` inlined.
fn is_unknown_tag_inside_liquid_block(tag: &LiquidTag<'_>, cx: &LiquidCx<'_, '_>) -> bool {
    if cx.source().as_bytes()[tag.position.start as usize..].starts_with(b"{%") {
        return false;
    }
    if matches!(tag.name, "#" | "else" | "elsif" | "when") {
        return false;
    }
    match cx.docset() {
        Some(docset) => docset.tag(tag.name).is_none(),
        // `Object.keys(builtinTags)`.
        None => matches!(tag_for_name(tag.name), None | Some(TagDefinition::ObjectPrototype)),
    }
}

fn check_assign(tag: &LiquidTag<'_>, raw: &str, cx: &LiquidCx<'_, '_>) {
    const SYNTAX_ERROR: &str = "Syntax error in 'assign' tag";
    let accepted = |m: &str| {
        has_ruby_accepted_empty_assign_rhs(m)
            || has_ruby_accepted_empty_first_filter_argument(m)
            || has_ruby_accepted_filter_argument_trailing_comma(m)
            || has_ruby_accepted_assign_lhs_extra_identifier(m)
    };
    let markup = match &tag.markup {
        Markup::Str(s) => {
            if !accepted(s) {
                report(cx, tag.position, SYNTAX_ERROR);
            }
            return;
        }
        Markup::Node(n) => &**n,
        _ => return,
    };
    let Node::AssignMarkup(assign) = markup else { return };
    if variable_has_bare_array_access(&assign.value) {
        report(cx, tag.position, BARE);
        return;
    }
    let source = cx.source();
    let start = assign.position.start;
    if has_skipped_prefix_characters(source, tag.markup_position.start, start) {
        report(cx, tag.position, SYNTAX_ERROR);
        return;
    }
    if accepted(raw) {
        return;
    }
    if has_skipped_characters(slice(source, start, tag.markup_position.end)) {
        report(cx, tag.position, SYNTAX_ERROR);
    }
}

fn check_block(tag: &LiquidTag<'_>, raw: &str, cx: &LiquidCx<'_, '_>) {
    const SYNTAX_ERROR: &str = "Syntax error in 'block' tag";
    let p = tag.position;
    let markup = match &tag.markup {
        Markup::Str(_) => return report(cx, p, SYNTAX_ERROR),
        Markup::Node(n) => &**n,
        _ => return,
    };
    let Node::BlockMarkup(markup) = markup else { return };
    let named = || {
        markup.args.iter().filter_map(|a| match a {
            Node::NamedArgument(a) => Some(&**a),
            _ => None,
        })
    };
    if let Node::String(name) = &markup.name {
        if name.value.starts_with("shopify://apps/") {
            if named().any(|a| a.name != "block.name") {
                return report(cx, p, "Liquid syntax error: in 'block' - app blocks do not accept arguments");
            }
            if has_content(tag) {
                return report(cx, p, "Liquid syntax error: in 'block' - app blocks do not accept content");
            }
        }
    }
    if let Some(dotted) = named().find(|a| a.name != "block.name" && a.name.contains('.')) {
        return report(cx, dotted.position, &dotted_argument_message(&dotted.name));
    }
    if named().any(|a| a.name == "block.name" && !matches!(a.value, Node::String(_))) {
        return report(cx, p, SYNTAX_ERROR);
    }
    if named().any(|a| has_bare_array_access(&a.value)) {
        return report(cx, p, BARE_STRICT2);
    }
    if has_skipped_characters(raw) {
        report(cx, p, SYNTAX_ERROR);
    }
}

fn has_content(tag: &LiquidTag<'_>) -> bool {
    tag.children.as_deref().unwrap_or(&[]).iter().any(|child| match child {
        Node::TextNode(t) => !trim(&t.value).is_empty(),
        _ => true,
    })
}

fn dotted_argument_message(name: &str) -> String {
    let mut parts = name.split('.');
    let (object, property, setting_id, rest) = (parts.next(), parts.next(), parts.next(), parts.next());
    match setting_id {
        Some(id) if object == Some("block") && property == Some("settings") && rest.is_none() && !id.is_empty() => {
            format!("Liquid syntax error: in 'block' - Use '{id}:' instead of '{name}:' when calling a block.")
        }
        _ => "Liquid syntax error: in 'block' - Use plain named arguments, for example: block 'name', heading: value".to_string(),
    }
}

/// `checkCaptureTag` and `checkDecrementTag`.
fn check_simple_lookup(tag: &LiquidTag<'_>, raw: &str, cx: &LiquidCx<'_, '_>, syntax_error: &str) {
    match &tag.markup {
        Markup::Str(_) => report(cx, tag.position, syntax_error),
        Markup::Node(n) if has_bare_array_access(n) => report(cx, tag.position, BARE),
        _ => {
            if has_skipped_characters(raw) {
                report(cx, tag.position, syntax_error);
            }
        }
    }
}

fn check_increment(tag: &LiquidTag<'_>, raw: &str, cx: &LiquidCx<'_, '_>) {
    const SYNTAX_ERROR: &str = "Syntax error in 'increment' tag";
    let bad = match &tag.markup {
        Markup::Str(_) => true,
        Markup::Node(n) => has_bare_array_access(n) || has_skipped_characters(raw),
        _ => has_skipped_characters(raw),
    };
    if bad {
        report(cx, tag.position, SYNTAX_ERROR);
    }
}

fn check_case(tag: &LiquidTag<'_>, raw: &str, cx: &LiquidCx<'_, '_>) {
    const SYNTAX_ERROR: &str = "Syntax error in 'case' tag";
    let markup = match &tag.markup {
        Markup::Str(_) => return report(cx, tag.position, SYNTAX_ERROR),
        Markup::Node(n) => &**n,
        _ => return,
    };
    if has_unclosed_quoted_string(raw) {
        return report(cx, tag.position, SYNTAX_ERROR);
    }
    if has_bare_array_access(markup) {
        return report(cx, tag.position, BARE);
    }
    if has_skipped_characters(slice(cx.source(), markup.position().start, tag.markup_position.end)) {
        report(cx, tag.position, SYNTAX_ERROR);
    }
}

fn check_content_for(tag: &LiquidTag<'_>, cx: &LiquidCx<'_, '_>) {
    const SYNTAX_ERROR: &str = "Syntax error in 'content_for' tag";
    let markup = match &tag.markup {
        Markup::Str(_) => return report(cx, tag.position, SYNTAX_ERROR),
        Markup::Node(n) => &**n,
        _ => return,
    };
    let Node::ContentForMarkup(markup) = markup else { return };
    let Node::String(content_for_type) = &markup.content_for_type else { return };
    let kind = content_for_type.value;
    if kind != "block" && kind != "blocks" {
        let message = format!("Invalid content_for type \"{kind}\"; expected \"block\" or \"blocks\"");
        return report(cx, tag.position, &message);
    }
    let invalid = has_invalid_shared_arguments(&markup.args)
        || if kind == "block" {
            has_invalid_block_arguments(&markup.args)
        } else {
            has_invalid_blocks_arguments(&markup.args)
        };
    if invalid {
        report(cx, tag.position, SYNTAX_ERROR);
    }
}

fn named_args<'n, 's>(args: &'n [Node<'s>]) -> impl Iterator<Item = &'n lq_syntax::NamedArgument<'s>> {
    args.iter().filter_map(|a| match a {
        Node::NamedArgument(a) => Some(&**a),
        _ => None,
    })
}

fn has_invalid_shared_arguments(args: &[Node<'_>]) -> bool {
    let has_duplicate = named_args(args)
        .enumerate()
        .any(|(i, a)| named_args(args).take(i).any(|b| b.name == a.name));
    has_duplicate || args.iter().any(arg_has_bare_array_access)
}

fn has_invalid_block_arguments(args: &[Node<'_>]) -> bool {
    if named_args(args).any(|a| a.name == "block" || a.name == "schema") {
        return true;
    }
    // `hasInvalidBlockStaticArguments`.
    let string_value = |name: &str| {
        named_args(args).find(|a| a.name == name).and_then(|a| match &a.value {
            Node::String(s) if !s.value.is_empty() => Some(()),
            _ => None,
        })
    };
    if string_value("type").is_none() || string_value("id").is_none() {
        return true;
    }
    // `hasInvalidBlockClosestArguments`.
    let mut context_argument_count = 0;
    for arg in named_args(args) {
        if !arg.name.starts_with("closest.") {
            continue;
        }
        if matches!(arg.value, Node::String(_)) {
            return true;
        }
        context_argument_count += 1;
        if context_argument_count > 1 || !is_supported_closest_argument(&arg.name) {
            return true;
        }
    }
    false
}

fn has_invalid_blocks_arguments(args: &[Node<'_>]) -> bool {
    let mut context_argument_count = 0;
    for arg in named_args(args) {
        if !arg.name.starts_with("closest.") || matches!(arg.value, Node::String(_)) {
            return true;
        }
        context_argument_count += 1;
        if context_argument_count > 1 || !is_supported_closest_argument(&arg.name) {
            return true;
        }
    }
    false
}

fn is_supported_closest_argument(name: &str) -> bool {
    let closest_type = &name["closest.".len()..];
    matches!(closest_type, "article" | "blog" | "collection" | "page" | "product")
        || closest_type.starts_with("metaobject.")
}

fn check_cycle(tag: &LiquidTag<'_>, raw: &str, cx: &LiquidCx<'_, '_>) {
    const SYNTAX_ERROR: &str = "Syntax error in 'cycle' tag";
    let markup = match &tag.markup {
        Markup::Str(s) => {
            if !has_ruby_accepted_cycle_trailing_comma(s) {
                report(cx, tag.position, SYNTAX_ERROR);
            }
            return;
        }
        Markup::Node(n) => &**n,
        _ => return,
    };
    let Node::CycleMarkup(markup) = markup else { return };
    if markup.group_name.as_ref().is_some_and(has_bare_array_access)
        || markup.args.iter().any(has_bare_array_access)
    {
        return report(cx, tag.position, BARE);
    }
    if has_skipped_characters(raw) {
        report(cx, tag.position, SYNTAX_ERROR);
    }
}

fn check_echo(tag: &LiquidTag<'_>, raw: &str, cx: &LiquidCx<'_, '_>) {
    const SYNTAX_ERROR: &str = "Syntax error in 'echo' tag";
    let markup = match &tag.markup {
        Markup::Str(s) => {
            if !has_empty_markup(raw)
                && !has_ruby_accepted_empty_first_filter_argument(s)
                && !has_ruby_accepted_filter_argument_trailing_comma(s)
            {
                report(cx, tag.position, SYNTAX_ERROR);
            }
            return;
        }
        Markup::Node(n) => &**n,
        _ => return,
    };
    if variable_has_bare_array_access(markup) {
        return report(cx, tag.position, BARE);
    }
    if !has_ruby_accepted_empty_first_filter_argument(raw)
        && !has_ruby_accepted_filter_argument_trailing_comma(raw)
        && has_skipped_characters(raw)
    {
        report(cx, tag.position, SYNTAX_ERROR);
    }
}

fn check_form(tag: &LiquidTag<'_>, raw: &str, cx: &LiquidCx<'_, '_>) {
    const SYNTAX_ERROR: &str = "Syntax error in 'form' tag";
    let args = match &tag.markup {
        Markup::Str(_) => return report(cx, tag.position, SYNTAX_ERROR),
        Markup::List(l) => l,
        _ => return,
    };
    if args.iter().any(arg_has_bare_array_access) {
        return report(cx, tag.position, BARE_STRICT2);
    }
    if has_invalid_form_argument_order(args) || has_skipped_characters(raw) {
        report(cx, tag.position, SYNTAX_ERROR);
    }
}

fn has_invalid_form_argument_order(args: &[Node<'_>]) -> bool {
    let is_named = |a: &Node<'_>| matches!(a, Node::NamedArgument(_));
    if args.first().is_none_or(is_named) {
        return true;
    }
    let mut positional_count = 1;
    let mut seen_named_argument = false;
    for arg in &args[1..] {
        if is_named(arg) {
            seen_named_argument = true;
            continue;
        }
        positional_count += 1;
        if seen_named_argument || positional_count > 2 {
            return true;
        }
    }
    false
}

/// `checkForTag` and `checkTablerowTag`.
fn check_loop(tag: &LiquidTag<'_>, cx: &LiquidCx<'_, '_>, syntax_error: &str, tablerow: bool) {
    let markup = match &tag.markup {
        Markup::Str(s) => {
            if !(tablerow && has_ruby_accepted_loop_trailing_comma(s)) {
                report(cx, tag.position, syntax_error);
            }
            return;
        }
        Markup::Node(n) => &**n,
        _ => return,
    };
    let Node::ForMarkup(for_markup) = markup else { return };
    if has_bare_array_access(&for_markup.collection)
        || (tablerow && for_markup.args.iter().any(arg_has_bare_array_access))
    {
        return report(cx, tag.position, BARE);
    }
    if has_skipped_around(cx.source(), tag.markup_position, for_markup.position.start) {
        report(cx, tag.position, syntax_error);
    }
}

/// `hasSkippedPrefixCharacters` before the parsed markup, `hasSkippedCharacters` from it on.
fn has_skipped_around(source: &str, markup_position: Position, parsed_start: u32) -> bool {
    has_skipped_prefix_characters(source, markup_position.start, parsed_start)
        || has_skipped_characters(slice(source, parsed_start, markup_position.end))
}

/// `checkIfTag` and `checkUnlessTag`.
fn check_condition(tag: &LiquidTag<'_>, raw: &str, cx: &LiquidCx<'_, '_>, syntax_error: &str) {
    let markup = match &tag.markup {
        Markup::Str(_) => return report(cx, tag.position, syntax_error),
        Markup::Node(n) => &**n,
        _ => return,
    };
    if has_unclosed_quoted_string(raw) {
        return report(cx, tag.position, syntax_error);
    }
    if conditional_has_bare_array_access(markup) {
        return report(cx, tag.position, BARE_STRICT2);
    }
    if has_skipped_around(cx.source(), tag.markup_position, markup.position().start) {
        report(cx, tag.position, syntax_error);
    }
}

fn render_variable_is_bare(variable: Option<&Node<'_>>) -> bool {
    matches!(variable, Some(Node::RenderVariableExpression(v)) if has_bare_array_access(&v.name))
}

fn check_include(tag: &LiquidTag<'_>, raw: &str, cx: &LiquidCx<'_, '_>) {
    const SYNTAX_ERROR: &str = "Syntax error in 'include' tag";
    let markup = match &tag.markup {
        Markup::Str(s) => {
            if !has_ruby_accepted_include_markup(s) {
                report(cx, tag.position, SYNTAX_ERROR);
            }
            return;
        }
        Markup::Node(n) => &**n,
        _ => return,
    };
    let Node::RenderMarkup(markup) = markup else { return };
    if has_bare_array_access(&markup.snippet)
        || render_variable_is_bare(markup.variable.as_ref())
        || markup.args.iter().any(arg_has_bare_array_access)
    {
        return report(cx, tag.position, BARE_STRICT2);
    }
    if has_skipped_characters(raw) {
        report(cx, tag.position, SYNTAX_ERROR);
    }
}

fn check_layout(tag: &LiquidTag<'_>, raw: &str, cx: &LiquidCx<'_, '_>) {
    const SYNTAX_ERROR: &str = "Syntax error in 'layout' tag";
    match &tag.markup {
        Markup::Str(_) => report(cx, tag.position, SYNTAX_ERROR),
        Markup::Node(n) if has_bare_array_access(n) => report(cx, tag.position, BARE_STRICT2),
        _ => {
            if has_skipped_characters(raw) {
                report(cx, tag.position, SYNTAX_ERROR);
            }
        }
    }
}

fn check_paginate(tag: &LiquidTag<'_>, cx: &LiquidCx<'_, '_>) {
    const SYNTAX_ERROR: &str = "Syntax error in 'paginate' tag";
    let p = tag.block_start_position;
    let markup = match &tag.markup {
        Markup::Str(s) => {
            if !has_ruby_accepted_paginate_trailing_comma(s) {
                report(cx, p, SYNTAX_ERROR);
            }
            return;
        }
        Markup::Node(n) => &**n,
        _ => return,
    };
    let Node::PaginateMarkup(markup) = markup else { return };
    if has_bare_array_access(&markup.collection)
        || has_bare_array_access(&markup.page_size)
        || markup.args.iter().any(arg_has_bare_array_access)
    {
        return report(cx, p, BARE);
    }
    if has_skipped_around(cx.source(), tag.markup_position, markup.position.start) {
        report(cx, p, SYNTAX_ERROR);
    }
}

fn check_partial(tag: &LiquidTag<'_>, raw: &str, cx: &LiquidCx<'_, '_>) {
    const SYNTAX_ERROR: &str = "Syntax error in 'partial' tag";
    let markup = match &tag.markup {
        Markup::Str(_) => return report(cx, tag.position, SYNTAX_ERROR),
        Markup::Node(n) => &**n,
        _ => return,
    };
    let Node::String(name) = markup else { return };
    if name.value.is_empty() || name.value.contains('/') {
        return report(cx, tag.position, "Liquid syntax error: Error in tag 'partial' - Valid syntax: partial '[name]'");
    }
    if has_skipped_characters(raw) {
        report(cx, tag.position, SYNTAX_ERROR);
    }
}

fn check_render(tag: &LiquidTag<'_>, raw: &str, cx: &LiquidCx<'_, '_>) {
    const SYNTAX_ERROR: &str = "Syntax error in 'render' tag";
    let markup = match &tag.markup {
        Markup::Str(_) => return report(cx, tag.position, SYNTAX_ERROR),
        Markup::Node(n) => &**n,
        _ => return,
    };
    let Node::RenderMarkup(markup) = markup else { return };
    if has_unclosed_quoted_string(raw) {
        return report(cx, tag.position, SYNTAX_ERROR);
    }
    if has_bare_array_access(&markup.snippet)
        || render_variable_is_bare(markup.variable.as_ref())
        || markup.args.iter().any(arg_has_bare_array_access)
    {
        return report(cx, tag.position, BARE_STRICT2);
    }
    if has_skipped_around(cx.source(), tag.markup_position, markup.position.start) {
        report(cx, tag.position, SYNTAX_ERROR);
    }
}

fn check_section(tag: &LiquidTag<'_>, raw: &str, cx: &LiquidCx<'_, '_>) {
    const SYNTAX_ERROR: &str = "Syntax error in 'section' tag";
    let p = tag.block_start_position;
    let markup = match &tag.markup {
        Markup::Str(_) => return report(cx, p, SYNTAX_ERROR),
        Markup::Node(n) => &**n,
        _ => return,
    };
    let Node::SectionMarkup(markup) = markup else { return };
    if named_args(&markup.args).any(|a| has_bare_array_access(&a.value)) {
        return report(cx, p, BARE_STRICT2);
    }
    if has_skipped_characters(raw) {
        report(cx, p, SYNTAX_ERROR);
    }
}

/// `checkBranchTag`.
fn check_branch(branch: &LiquidBranch<'_>, cx: &LiquidCx<'_, '_>) {
    let p = branch.block_start_position;
    let source = cx.source();
    let raw = || slice(source, branch.markup_position.start, branch.markup_position.end);
    match branch.name {
        Some("elsif") => {
            const SYNTAX_ERROR: &str = "Syntax error in 'elsif' tag";
            let markup = match &branch.markup {
                Markup::Str(_) => return report(cx, p, SYNTAX_ERROR),
                Markup::Node(n) => &**n,
                _ => return,
            };
            if has_unclosed_quoted_string(raw()) {
                return report(cx, p, SYNTAX_ERROR);
            }
            if conditional_has_bare_array_access(markup) {
                return report(cx, p, BARE_STRICT2);
            }
            if has_skipped_around(source, branch.markup_position, markup.position().start) {
                report(cx, p, SYNTAX_ERROR);
            }
        }
        Some("when") => {
            const SYNTAX_ERROR: &str = "Syntax error in 'when' tag";
            let markup = match &branch.markup {
                Markup::Str(_) => return report(cx, p, SYNTAX_ERROR),
                Markup::List(l) => l,
                _ => return,
            };
            if has_unclosed_quoted_string(raw()) {
                return report(cx, p, SYNTAX_ERROR);
            }
            if markup.iter().any(has_bare_array_access) {
                return report(cx, p, BARE);
            }
            let Some(first) = markup.first() else { return };
            if has_skipped_characters(slice(source, first.position().start, branch.markup_position.end)) {
                report(cx, p, SYNTAX_ERROR);
            }
        }
        _ => {}
    }
}

/// `checkVariableOutput`.
fn check_variable_output(output: &LiquidVariableOutput<'_>, cx: &LiquidCx<'_, '_>) {
    const SYNTAX_ERROR: &str = "Syntax error in variable output";
    let raw = slice(cx.source(), output.markup_position.start, output.markup_position.end);
    let markup = match &output.markup {
        Markup::Str(s) => {
            if !has_empty_markup(raw)
                && !has_ruby_accepted_empty_first_filter_argument(s)
                && !has_ruby_accepted_filter_argument_trailing_comma(s)
            {
                report(cx, output.position, SYNTAX_ERROR);
            }
            return;
        }
        Markup::Node(n) => &**n,
        _ => return,
    };
    if has_ruby_accepted_empty_first_filter_argument(raw)
        || has_ruby_accepted_filter_argument_trailing_comma(raw)
    {
        return;
    }
    if has_unclosed_quoted_string(raw) {
        return report(cx, output.position, SYNTAX_ERROR);
    }
    if variable_has_bare_array_access(markup) {
        return report(cx, output.position, BARE_STRICT2);
    }
    if has_skipped_characters(raw) {
        report(cx, output.position, SYNTAX_ERROR);
    }
}

/// `checkCommentRawTag`. Without a `{%` or `{{` the body has no tag or delimiter
/// tokens, so nothing here can fire.
fn check_comment_raw_tag(tag: &LiquidRawTag<'_>, cx: &LiquidCx<'_, '_>) {
    let Node::RawMarkup(body) = &tag.body else { return };
    let value = body.value;
    if !value.contains("{%") && !value.contains("{{") {
        return;
    }
    let tokens = tokenize(value, TokenizeOptions::default());
    let source = cx.source();
    let after = tag.block_end_position.end;
    if has_unbalanced_nested_tags(value, &tokens, source, after, "comment")
        || has_unbalanced_nested_tags(value, &tokens, source, after, "raw")
        || has_unclosed_liquid_delimiter(&tokens)
    {
        report(cx, body.position, COMMENT_RAW_BODY_ERROR);
    }
}

fn has_unclosed_liquid_delimiter(tokens: &[lq_syntax::Token]) -> bool {
    for (i, open) in tokens.iter().enumerate() {
        let text = tokens.get(i + 1);
        let close = if text.is_some_and(|t| t.token_type == TokenType::Text) {
            tokens.get(i + 2)
        } else {
            text
        };
        let close_type = close.map(|c| c.token_type);
        if open.token_type == TokenType::LiquidTagOpen && close_type != Some(TokenType::LiquidTagClose) {
            return true;
        }
        if open.token_type == TokenType::LiquidVariableOutputOpen
            && close_type != Some(TokenType::LiquidVariableOutputClose)
        {
            return true;
        }
    }
    false
}

fn has_unbalanced_nested_tags(
    body: &str,
    body_tokens: &[lq_syntax::Token],
    source: &str,
    source_after_closing_tag: u32,
    tag_name: &str,
) -> bool {
    let mut depth = 0;
    for tag in liquid_tag_bodies_in(body, body_tokens, 0) {
        let Some(markup) = liquid_tag_markup(tag.body) else { continue };
        if markup.has_skipped_characters {
            continue;
        }
        if markup.tag_name == tag_name {
            depth += 1;
        } else if is_end_tag(markup.tag_name, tag_name) && depth > 0 {
            depth -= 1;
        }
    }
    if depth == 0 {
        return false;
    }
    for tag in liquid_tag_bodies(source, source_after_closing_tag) {
        let Some(markup) = liquid_tag_markup(tag.body) else { continue };
        if markup.has_skipped_characters {
            continue;
        }
        if markup.tag_name == tag_name {
            depth += 1;
        } else if is_end_tag(markup.tag_name, tag_name) {
            depth -= 1;
            if depth == 0 {
                return false;
            }
        }
    }
    true
}

/// `countLiquidTag`.
fn count_liquid_tag(source: &str, tag_name: &str, start_index: u32, end_index: u32) -> usize {
    liquid_tag_bodies(source, start_index)
        .iter()
        .filter(|tag| {
            tag.body_start < end_index
                && liquid_tag_markup(tag.body)
                    .is_some_and(|m| m.tag_name == tag_name && !m.has_skipped_characters)
        })
        .count()
}

/// `hasRubyAcceptedInertCommentBodyCloser`.
fn has_ruby_accepted_inert_comment_body_closer(source: &str, tag_name: &str, start_index: u32) -> bool {
    let closer = liquid_tag_at(source, start_index);
    let Some(closer) = closer else { return false };
    if !is_end_tag(closer.tag_name, tag_name) || closer.has_skipped_characters {
        return false;
    }
    let end = source.len() as u32;
    if tag_name == "comment" {
        return count_liquid_tag(source, "comment", 0, start_index)
            > count_liquid_tag(source, "endcomment", 0, start_index);
    }
    count_liquid_tag(source, "raw", 0, start_index) > count_liquid_tag(source, "endraw", 0, start_index)
        && count_liquid_tag(source, "comment", 0, start_index) > 0
        && count_liquid_tag(source, "endcomment", start_index, end) > 0
}

/// `liquidTagAt`.
fn liquid_tag_at(source: &str, start_index: u32) -> Option<LiquidTagMarkup<'_>> {
    let tag = liquid_tag_bodies(source, start_index).into_iter().next()?;
    (tag.start == start_index).then(|| liquid_tag_markup(tag.body)).flatten()
}

/// The body of a raw tag is everything after its opening `{% ... %}`.
fn raw_tag_body(source: &str, start_index: u32) -> &str {
    let bytes = source.as_bytes();
    let from = (start_index as usize).min(bytes.len());
    match bytes[from..].windows(2).position(|w| w == b"%}") {
        Some(i) => &source[from + i + 2..],
        None => "",
    }
}

/// `checkCommentTag`.
fn check_comment_parser_error(error: &ParseError, cx: &LiquidCx<'_, '_>, source: &str) {
    let message = error.message.as_str();
    if message != UNCLOSED_COMMENT && message != UNMATCHED_COMMENT_CLOSE {
        return;
    }
    let end = source.len() as u32;
    if count_liquid_tag(source, "comment", 0, end) == 0 && count_liquid_tag(source, "endcomment", 0, end) == 0 {
        return;
    }
    let location = resolve_error_location(error, source);
    let start_index = location.0;
    if message == UNCLOSED_COMMENT && has_nested_comment_tag(raw_tag_body(source, start_index)) {
        return report_at(cx, location, COMMENT_RAW_BODY_ERROR);
    }
    if message == UNMATCHED_COMMENT_CLOSE
        && has_ruby_accepted_inert_comment_body_closer(source, "comment", start_index)
    {
        return;
    }
    report_at(cx, location, message);
}

fn has_nested_comment_tag(body: &str) -> bool {
    liquid_tag_bodies(body, 0).iter().any(|tag| {
        liquid_tag_markup(tag.body).is_some_and(|m| m.tag_name == "comment" && !m.has_skipped_characters)
    })
}

/// `checkBlockParserError`.
fn check_block_parser_error(error: &ParseError, cx: &LiquidCx<'_, '_>, source: &str) {
    let message = error.message.as_str();
    if message != UNCLOSED_BLOCK && message != UNCLOSED_BLOCK_IN_LIQUID && message != UNOPENED_BLOCK {
        return;
    }
    let location = if message == UNCLOSED_BLOCK_IN_LIQUID {
        let location = if cx.divergence(Divergence::LiquidSyntaxUnclosedLine) {
            unclosed_liquid_statement(source, 0, "block")
        } else {
            liquid_line_tag_location(source, "block")
        };
        location.unwrap_or_else(|| resolve_error_location(error, source))
    } else {
        resolve_error_location(error, source)
    };
    let shown = if message == UNCLOSED_BLOCK { "Liquid syntax error: 'block' tag was never closed" } else { message };
    report_at(cx, location, shown);
}

/// `checkPartialParserError`.
fn check_partial_parser_error(error: &ParseError, cx: &LiquidCx<'_, '_>, source: &str) {
    let message = error.message.as_str();
    if message != UNCLOSED_PARTIAL && message != UNCLOSED_PARTIAL_IN_LIQUID && message != UNOPENED_PARTIAL {
        return;
    }
    let location = if message == UNCLOSED_PARTIAL_IN_LIQUID {
        let location = if cx.divergence(Divergence::LiquidSyntaxUnclosedLine) {
            unclosed_liquid_statement(source, 0, "partial")
        } else {
            liquid_line_tag_location(source, "partial")
        };
        location.unwrap_or_else(|| resolve_error_location(error, source))
    } else {
        resolve_error_location(error, source)
    };
    let shown = if message == UNCLOSED_PARTIAL { "Liquid syntax error: 'partial' tag was never closed" } else { message };
    report_at(cx, location, shown);
}

/// `checkDocTag`.
fn check_doc_tag(tag: &LiquidRawTag<'_>, cx: &LiquidCx<'_, '_>) {
    if !tag.markup.is_empty() {
        return report(cx, tag.block_start_position, "Syntax error in 'doc' tag");
    }
    let Node::RawMarkup(body) = &tag.body else { return };
    if body.value.contains("{%") && has_nested_doc_tag(body.value) {
        report(cx, body.position, "Nested doc tags are not allowed");
    }
}

fn has_nested_doc_tag(body: &str) -> bool {
    for tag in liquid_tag_bodies(body, 0) {
        let Some(markup) = liquid_tag_markup(tag.body) else { continue };
        if markup.tag_name == "enddoc" && markup.has_remaining_tokens && !markup.has_skipped_characters {
            return false;
        }
        if markup.tag_name == "doc" && !markup.has_skipped_characters {
            return true;
        }
    }
    false
}

/// `checkDocParserError`.
fn check_doc_parser_error(error: &ParseError, cx: &LiquidCx<'_, '_>, source: &str) {
    let message = error.message.as_str();
    if message != UNCLOSED_DOC && message != UNOPENED_DOC {
        return;
    }
    if !has_liquid_tag_named(source, "doc") && !has_liquid_tag_named(source, "enddoc") {
        return;
    }
    let location = resolve_error_location(error, source);
    if message == UNCLOSED_DOC {
        if has_nested_doc_tag(raw_tag_body(source, location.0)) {
            return report_at(cx, location, "Nested doc tags are not allowed");
        }
        if has_ruby_accepted_raw_tag_closer_with_markup(source, "doc", location.0) {
            return;
        }
    }
    report_at(cx, location, message);
}

/// `checkRawParserError`.
fn check_raw_parser_error(error: &ParseError, cx: &LiquidCx<'_, '_>, source: &str) {
    let message = error.message.as_str();
    if message != UNCLOSED_RAW && message != UNMATCHED_RAW_CLOSE && message != UNCLOSED_RAW_IN_LIQUID {
        return;
    }
    let location = resolve_error_location(error, source);
    if message == UNCLOSED_RAW && has_ruby_accepted_raw_tag_closer_with_markup(source, "raw", location.0) {
        return;
    }
    if message == UNMATCHED_RAW_CLOSE && has_ruby_accepted_inert_comment_body_closer(source, "raw", location.0) {
        return;
    }
    report_at(cx, location, message);
}

/// `checkJavascriptParserError`.
fn check_javascript_parser_error(error: &ParseError, cx: &LiquidCx<'_, '_>, source: &str) {
    let message = error.message.as_str();
    if message != UNOPENED_JAVASCRIPT && message != UNCLOSED_JAVASCRIPT {
        return;
    }
    // `hasJavascriptParserTag`.
    let tags = liquid_tag_bodies(source, 0);
    let markups: Vec<_> = tags.iter().filter_map(|tag| Some((tag, liquid_tag_markup(tag.body)?))).collect();
    if !markups.iter().any(|(_, m)| m.tag_name == "javascript" || m.tag_name == "endjavascript") {
        return;
    }
    if message == UNOPENED_JAVASCRIPT {
        return report_at(cx, resolve_error_location(error, source), "Unknown tag 'endjavascript'");
    }
    let invalid_opening = markups.iter().find(|(_, m)| {
        m.tag_name == "javascript" && (m.has_remaining_tokens || m.has_skipped_characters)
    });
    if let Some((tag, _)) = invalid_opening {
        return report_at(cx, (tag.start, tag.end), JAVASCRIPT_SYNTAX_ERROR);
    }
    let location = resolve_error_location(error, source);
    // `hasJavascriptClosingTagAfter`: the local raw-tag scanner only recognizes bare closers.
    let has_closer = markups.iter().any(|(tag, m)| m.tag_name == "endjavascript" && tag.start >= location.0);
    if !has_closer {
        report_at(cx, location, "'javascript' tag was never closed");
    }
}
