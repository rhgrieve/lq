//! Port of `printer/preprocess/*`. Each `augmentWith*` fills `Ctx::info` instead of
//! assigning to the node.

use super::string::{is_js_ws, is_whitespace_at, is_whitespace_before};
use super::utils::*;
use super::*;
use crate::constants::*;
use crate::options::{CaptureWhitespaceSensitivity, HtmlWhitespaceSensitivity};
use lq_syntax::{Markup, Node};
use std::cell::Cell;

/// Calls `list` for each array field and `one` for each single node field, in the key
/// order of upstream's factories. Mirrors `walk`: a `BlockArrayLiteral` value is skipped.
fn fields<'a>(
    node: N<'a>,
    list: &mut dyn FnMut(&'a [Node<'a>], bool),
    one: &mut dyn FnMut(N<'a>),
) {
    fn markup<'a>(
        m: &'a Markup<'a>,
        list: &mut dyn FnMut(&'a [Node<'a>], bool),
        one: &mut dyn FnMut(N<'a>),
    ) {
        match m {
            Markup::Node(c) => one(c),
            Markup::List(l) => list(l, false),
            Markup::Str(_) | Markup::None => {}
        }
    }
    match node {
        Node::Document(n) => list(&n.children, false),
        Node::LiquidRawTag(n) => one(&n.body),
        Node::LiquidTag(n) => {
            markup(&n.markup, list, one);
            if let Some(c) = &n.children {
                list(c, false);
            }
        }
        Node::LiquidBranch(n) => {
            markup(&n.markup, list, one);
            list(&n.children, false);
        }
        Node::LiquidVariableOutput(n) => markup(&n.markup, list, one),
        Node::HtmlSelfClosingElement(n) => {
            list(&n.name, false);
            list(&n.attributes, true);
        }
        Node::HtmlVoidElement(n) => list(&n.attributes, true),
        Node::HtmlElement(n) => {
            list(&n.name, false);
            list(&n.attributes, true);
            list(&n.children, false);
        }
        Node::HtmlDanglingMarkerClose(n) => list(&n.name, false),
        Node::HtmlRawNode(n) => {
            list(&n.attributes, true);
            one(&n.body);
        }
        Node::AttrSingleQuoted(n) | Node::AttrDoubleQuoted(n) | Node::AttrUnquoted(n) => {
            list(&n.name, false);
            list(&n.value, false);
        }
        Node::AttrEmpty(n) => list(&n.name, false),
        Node::LiquidVariable(n) => {
            one(&n.expression);
            list(&n.filters, false);
        }
        Node::LiquidFilter(n) => list(&n.args, false),
        Node::NamedArgument(n) => {
            if !matches!(n.value, Node::BlockArrayLiteral(_)) {
                one(&n.value);
            }
        }
        Node::BooleanExpression(n) => one(&n.condition),
        Node::Range(n) => {
            one(&n.start);
            one(&n.end);
        }
        Node::VariableLookup(n) => list(&n.lookups, false),
        Node::Comparison(n) => {
            one(&n.left);
            one(&n.right);
        }
        Node::LogicalExpression(n) => {
            one(&n.left);
            one(&n.right);
        }
        Node::AssignMarkup(n) => one(&n.value),
        Node::BlockMarkup(n) => {
            one(&n.name);
            list(&n.args, false);
        }
        Node::ContentForMarkup(n) => {
            one(&n.content_for_type);
            list(&n.args, false);
        }
        Node::CycleMarkup(n) => {
            if let Some(c) = &n.group_name {
                one(c);
            }
            list(&n.args, false);
        }
        Node::ForMarkup(n) => {
            one(&n.collection);
            list(&n.args, false);
        }
        Node::PaginateMarkup(n) => {
            one(&n.collection);
            one(&n.page_size);
            list(&n.args, false);
        }
        Node::RawMarkup(n) => list(&n.nodes, false),
        Node::RenderMarkup(n) => {
            one(&n.snippet);
            if let Some(c) = &n.variable {
                one(c);
            }
            if let Some(c) = &n.alias {
                one(c);
            }
            list(&n.args, false);
        }
        Node::SectionMarkup(n) => {
            one(&n.name);
            list(&n.args, false);
        }
        Node::RenderVariableExpression(n) => one(&n.name),
        Node::LiquidDocParamNode(n) => {
            one(&n.param_name);
            if let Some(c) = &n.param_type {
                one(c);
            }
            if let Some(c) = &n.param_description {
                one(c);
            }
        }
        Node::LiquidDocDescriptionNode(n) => one(&n.content),
        Node::LiquidDocExampleNode(n) => one(&n.content),
        Node::LiquidDocPromptNode(n) => one(&n.content),
        Node::BlockArrayLiteral(n) => list(&n.elements, false),
        Node::YamlFrontmatter(_)
        | Node::HtmlDoctype(_)
        | Node::HtmlComment(_)
        | Node::TextNode(_)
        | Node::LiquidLiteral(_)
        | Node::String(_)
        | Node::Number(_)
        | Node::RenderAliasExpression(_)
        | Node::LiquidErrorNode(_) => {}
    }
}

/// `augmentWithParent`, `augmentWithSiblings` and `augmentWithFamily`.
fn augment_with_family<'a>(ctx: &mut Ctx<'a>, node: N<'a>, visited: &mut Vec<N<'a>>) {
    let id = node.id().0 as usize;
    ctx.nodes[id] = Some(node);
    visited.push(node);
    if let Some(children) = children_of(node) {
        if let (Some(f), Some(l)) = (children.first(), children.last()) {
            ctx.info[id].first_child = f.id().0;
            ctx.info[id].last_child = l.id().0;
        }
    }
    let mut pending: Vec<(&'a [Node<'a>], bool)> = Vec::new();
    let mut singles: Vec<N<'a>> = Vec::new();
    fields(node, &mut |l, a| pending.push((l, a)), &mut |n| singles.push(n));
    for c in singles {
        ctx.info[c.id().0 as usize].parent = id as u32;
        augment_with_family(ctx, c, visited);
    }
    for (list, is_attributes) in pending {
        for (i, c) in list.iter().enumerate() {
            let info = &mut ctx.info[c.id().0 as usize];
            info.parent = id as u32;
            if i > 0 {
                info.prev = list[i - 1].id().0;
            }
            if let Some(n) = list.get(i + 1) {
                info.next = n.id().0;
            }
            if is_attributes {
                info.flags |= IS_ATTRIBUTE;
            }
            augment_with_family(ctx, c, visited);
        }
    }
}

/// `/^\s*display:\s*([a-z]+)\s*$/` and `/^\s*white-?space:\s*([a-z]+)\s*$/`.
fn css_from_comment(body: &str, white_space: bool) -> Option<&str> {
    let rest = body.trim_start_matches(is_js_ws);
    let rest = if white_space {
        rest.strip_prefix("whitespace:").or_else(|| rest.strip_prefix("white-space:"))?
    } else {
        rest.strip_prefix("display:")?
    };
    let rest = rest.trim_start_matches(is_js_ws);
    let len = rest.bytes().take_while(u8::is_ascii_lowercase).count();
    if len == 0 || !rest[len..].chars().all(is_js_ws) {
        return None;
    }
    Some(&rest[..len])
}

fn comment_css<'a>(ctx: &Ctx<'a>, node: N<'a>, white_space: bool) -> Option<&'a str> {
    match ctx.prev(node)? {
        Node::HtmlComment(c) => css_from_comment(c.body, white_space),
        Node::LiquidTag(t) if t.name == "#" => match &t.markup {
            Markup::Str(m) => css_from_comment(m, white_space),
            _ => None,
        },
        _ => None,
    }
}

fn get_css_display<'a>(ctx: &Ctx<'a>, node: N<'a>) -> &'a str {
    if let Some(d) = comment_css(ctx, node, false) {
        return d;
    }
    let sensitivity = ctx.options.html_whitespace_sensitivity;
    match node {
        Node::HtmlElement(_) | Node::HtmlDanglingMarkerClose(_) | Node::HtmlSelfClosingElement(_) => {
            match sensitivity {
                HtmlWhitespaceSensitivity::Strict => "inline",
                HtmlWhitespaceSensitivity::Ignore => "block",
                HtmlWhitespaceSensitivity::Css => plain_compound_name(node)
                    .and_then(|name| {
                        if is_object_prototype_member(name) {
                            Some(FUNCTION_VALUE)
                        } else {
                            css_display_tags(name)
                        }
                    })
                    .unwrap_or(CSS_DISPLAY_DEFAULT),
            }
        }
        Node::HtmlVoidElement(_) | Node::HtmlRawNode(_) => match sensitivity {
            HtmlWhitespaceSensitivity::Strict => "inline",
            HtmlWhitespaceSensitivity::Ignore => "block",
            HtmlWhitespaceSensitivity::Css => {
                string_name(node).and_then(css_display_tags).unwrap_or(CSS_DISPLAY_DEFAULT)
            }
        },
        Node::RawMarkup(_) | Node::TextNode(_) => "inline",
        Node::LiquidTag(_) | Node::LiquidRawTag(_) => match sensitivity {
            HtmlWhitespaceSensitivity::Strict => "inline",
            HtmlWhitespaceSensitivity::Ignore => "block",
            HtmlWhitespaceSensitivity::Css => string_name(node)
                .and_then(css_display_liquid_tags)
                .unwrap_or(CSS_DISPLAY_LIQUID_DEFAULT),
        },
        Node::LiquidBranch(_) | Node::LiquidVariableOutput(_) => "inline",
        Node::AttrDoubleQuoted(_)
        | Node::AttrSingleQuoted(_)
        | Node::AttrUnquoted(_)
        | Node::AttrEmpty(_) => "inline",
        Node::HtmlDoctype(_) | Node::HtmlComment(_) => "block",
        Node::Document(_) => "block",
        Node::YamlFrontmatter(_) => "block",
        _ => "should not be relevant",
    }
}

fn get_css_white_space<'a>(ctx: &Ctx<'a>, node: N<'a>) -> &'a str {
    if let Some(w) = comment_css(ctx, node, true) {
        return w;
    }
    match node {
        Node::HtmlElement(_) | Node::HtmlDanglingMarkerClose(_) | Node::HtmlSelfClosingElement(_) => {
            plain_compound_name(node)
                .and_then(|name| {
                    if is_object_prototype_member(name) {
                        Some(FUNCTION_VALUE)
                    } else {
                        css_white_space_tags(name)
                    }
                })
                .unwrap_or(CSS_WHITE_SPACE_DEFAULT)
        }
        Node::HtmlVoidElement(_) | Node::HtmlRawNode(_) => {
            string_name(node).and_then(css_white_space_tags).unwrap_or(CSS_WHITE_SPACE_DEFAULT)
        }
        Node::TextNode(_) => CSS_WHITE_SPACE_DEFAULT,
        Node::RawMarkup(_) | Node::YamlFrontmatter(_) | Node::LiquidRawTag(_) => "pre",
        Node::LiquidTag(t) => {
            if t.name == "capture" {
                match ctx.options.capture_whitespace_sensitivity {
                    CaptureWhitespaceSensitivity::Strict => "pre",
                    CaptureWhitespaceSensitivity::Ignore => "normal",
                }
            } else {
                css_white_space_liquid_tags(t.name).unwrap_or(CSS_WHITE_SPACE_DEFAULT)
            }
        }
        Node::LiquidBranch(_) | Node::LiquidVariableOutput(_) => CSS_WHITE_SPACE_DEFAULT,
        Node::AttrDoubleQuoted(_)
        | Node::AttrSingleQuoted(_)
        | Node::AttrUnquoted(_)
        | Node::AttrEmpty(_) => CSS_WHITE_SPACE_DEFAULT,
        Node::HtmlDoctype(_) | Node::HtmlComment(_) => CSS_WHITE_SPACE_DEFAULT,
        Node::Document(_) => CSS_WHITE_SPACE_DEFAULT,
        _ => "should not be relevant",
    }
}

// --- augment-with-whitespace-helpers.ts ---

#[inline]
fn is_block_like_css_display(d: &str) -> bool {
    if d == FUNCTION_VALUE {
        note_type_error("cssDisplay.startsWith is not a function");
    }
    d == "block" || d == "list-item" || d.starts_with("table")
}

#[inline]
fn is_inner_left_space_sensitive_css_display(d: &str) -> bool {
    !is_block_like_css_display(d) && d != "inline-block"
}

#[inline]
fn is_inner_right_whitespace_sensitive_css_display(d: &str) -> bool {
    !is_block_like_css_display(d) && d != "inline-block"
}

#[inline]
fn is_outer_left_whitespace_sensitive_css_display(d: &str) -> bool {
    !is_block_like_css_display(d)
}

#[inline]
fn is_outer_right_whitespace_sensitive_css_display(d: &str) -> bool {
    !is_block_like_css_display(d)
}

#[inline]
fn is_dangling_space_sensitive_css_display(d: &str) -> bool {
    !is_block_like_css_display(d) && d != "inline-block"
}

fn creates_inline_formatting_context(d: &str) -> bool {
    is_block_like_css_display(d) || d == "inline" || d == "inline-block"
}

fn is_html_element_without_close_tag(node: N<'_>) -> bool {
    is_html_dangling_marker_open(node)
}

fn is_trimming_outer_right(node: Option<N<'_>>) -> bool {
    match node {
        Some(Node::LiquidRawTag(n)) => n.delimiter_whitespace_end == "-",
        Some(Node::LiquidTag(n)) => n.delimiter_whitespace_end.unwrap_or(n.whitespace_end) == "-",
        Some(Node::LiquidVariableOutput(n)) => n.whitespace_end == "-",
        _ => false,
    }
}

fn is_trimming_outer_left(node: Option<N<'_>>) -> bool {
    match node {
        Some(Node::LiquidRawTag(n)) => n.whitespace_start == "-",
        Some(Node::LiquidTag(n)) => n.whitespace_start == "-",
        Some(Node::LiquidBranch(n)) => n.whitespace_start == "-",
        Some(Node::LiquidVariableOutput(n)) => n.whitespace_start == "-",
        _ => false,
    }
}

fn is_trimming_inner_left<'a>(ctx: &Ctx<'a>, node: Option<N<'a>>) -> bool {
    match node {
        Some(Node::LiquidRawTag(n)) => n.whitespace_end == "-",
        Some(Node::LiquidTag(n)) => n.delimiter_whitespace_end.is_some() && n.whitespace_end == "-",
        Some(b @ Node::LiquidBranch(n)) => {
            let Some(parent @ Node::LiquidTag(_)) = ctx.parent(b) else {
                return false;
            };
            if ctx.prev(b).is_none() {
                return is_trimming_inner_left(ctx, Some(parent));
            }
            n.whitespace_end == "-"
        }
        _ => false,
    }
}

fn is_trimming_inner_right<'a>(ctx: &Ctx<'a>, node: Option<N<'a>>) -> bool {
    match node {
        Some(Node::LiquidRawTag(n)) => n.delimiter_whitespace_start == "-",
        Some(Node::LiquidTag(n)) => n.delimiter_whitespace_start == Some("-"),
        Some(b @ Node::LiquidBranch(_)) => {
            let Some(parent @ Node::LiquidTag(_)) = ctx.parent(b) else {
                return false;
            };
            match ctx.next(b) {
                None => is_trimming_inner_right(ctx, Some(parent)),
                next => is_trimming_outer_left(next),
            }
        }
        _ => false,
    }
}

fn is_dangling_whitespace_sensitive_node<'a>(ctx: &Ctx<'a>, node: N<'a>) -> bool {
    is_dangling_space_sensitive_css_display(ctx.get(node).css_display)
        && !is_script_like_tag(node)
        && !is_trimming_inner_left(ctx, Some(node))
        && !is_trimming_inner_right(ctx, Some(node))
}

fn get_node_css_style_white_space(node: N<'_>) -> &'static str {
    let from_html = if is_html_node(node) { string_name(node).and_then(css_white_space_tags) } else { None };
    from_html
        .or_else(|| {
            if is_liquid_node(node) {
                string_name(node).and_then(css_white_space_liquid_tags)
            } else {
                None
            }
        })
        .unwrap_or(CSS_WHITE_SPACE_DEFAULT)
}

fn is_indentation_sensitive_node(node: N<'_>) -> bool {
    get_node_css_style_white_space(node).starts_with("pre")
}

fn is_leading_whitespace_sensitive_node<'a>(ctx: &Ctx<'a>, node: Option<N<'a>>) -> bool {
    let Some(node) = node else {
        return false;
    };
    let parent = ctx.parent(node);

    if let Node::LiquidBranch(b) = node {
        let is_default_branch = b.name.is_none();
        // Upstream names this `hasNoChildren` though it holds the opposite.
        let has_no_children = ctx.first_child(node).is_some();
        let is_parent_inner_right_sensitive = is_inner_left_space_sensitive_css_display(
            parent.map_or("", |p| ctx.get(p).css_display),
        );
        let is_first_child_leading_sensitive =
            is_leading_whitespace_sensitive_node(ctx, ctx.first_child(node));
        if is_default_branch {
            return is_parent_inner_right_sensitive
                && (!has_no_children || is_first_child_leading_sensitive);
        }
        return is_parent_inner_right_sensitive;
    }

    if let Some(p) = parent {
        if ctx.has_flag(p, IS_ATTRIBUTE) && matches!(node, Node::LiquidVariableOutput(_)) {
            return true;
        }
    }

    if is_trimming_outer_left(Some(node)) {
        return false;
    }

    let prev = ctx.prev(node);
    if prev.is_some() && is_trimming_outer_right(prev) {
        return false;
    }

    let Some(parent) = parent else {
        return false;
    };
    if ctx.get(parent).css_display == "none" {
        return false;
    }

    if ctx.is_pre_like_node(parent) {
        return true;
    }

    if is_script_like_tag(node) {
        return false;
    }

    if prev.is_none()
        && (matches!(parent, Node::Document(_))
            || ctx.is_pre_like_node(node)
            || is_script_like_tag(parent)
            || !is_inner_left_space_sensitive_css_display(ctx.get(parent).css_display)
            || is_trimming_inner_left(ctx, Some(parent)))
    {
        return false;
    }

    if let Some(p) = prev {
        if !is_outer_right_whitespace_sensitive_css_display(ctx.get(p).css_display) {
            return false;
        }
    }

    if !is_outer_left_whitespace_sensitive_css_display(ctx.get(node).css_display) {
        return false;
    }

    true
}

fn is_trailing_whitespace_sensitive_node<'a>(ctx: &Ctx<'a>, node: N<'a>) -> bool {
    let parent = ctx.parent(node);

    if let Node::LiquidBranch(_) = node {
        let is_last_branch = parent
            .and_then(|p| ctx.last_child(p))
            .is_some_and(|l| Ctx::same(l, node));
        let last_child = ctx.last_child(node);
        let has_no_last_child = last_child.is_none();
        let is_last_child_trailing_sensitive =
            last_child.is_some_and(|l| is_trailing_whitespace_sensitive_node(ctx, l));
        if is_last_branch {
            let is_parent_inner_right_sensitive = is_inner_right_whitespace_sensitive_css_display(
                parent.map_or("", |p| ctx.get(p).css_display),
            );
            return is_parent_inner_right_sensitive
                && (has_no_last_child || is_last_child_trailing_sensitive);
        }
        return has_no_last_child || is_last_child_trailing_sensitive;
    }

    if is_html_element_without_close_tag(node) {
        return match ctx.last_child(node) {
            None => is_inner_left_space_sensitive_css_display(ctx.get(node).css_display),
            Some(l) => {
                creates_inline_formatting_context(ctx.get(node).css_display)
                    && is_trailing_whitespace_sensitive_node(ctx, l)
            }
        };
    }

    if is_trimming_outer_right(Some(node)) {
        return false;
    }

    if let Some(p) = parent {
        if ctx.has_flag(p, IS_ATTRIBUTE) && matches!(node, Node::LiquidVariableOutput(_)) {
            return true;
        }
    }

    let next = ctx.next(node);
    if next.is_some() && is_trimming_outer_left(next) {
        return false;
    }

    let Some(parent) = parent else {
        return false;
    };
    if ctx.get(parent).css_display == "none" {
        return false;
    }

    if ctx.is_pre_like_node(parent) {
        return true;
    }

    if is_script_like_tag(node) {
        return false;
    }

    if is_html_node(node) && string_name(node) == Some("br") {
        return false;
    }

    if next.is_none()
        && (matches!(parent, Node::Document(_))
            || ctx.is_pre_like_node(node)
            || is_script_like_tag(parent)
            || (!is_html_element_without_close_tag(parent)
                && !is_inner_right_whitespace_sensitive_css_display(ctx.get(parent).css_display))
            || is_trimming_inner_right(ctx, Some(parent))
            || ctx.has_flag(node, IS_ATTRIBUTE))
    {
        return false;
    }

    if let Some(n) = next {
        if !is_outer_left_whitespace_sensitive_css_display(ctx.get(n).css_display) {
            return false;
        }
    }

    if !is_outer_right_whitespace_sensitive_css_display(ctx.get(node).css_display) {
        return false;
    }

    true
}

fn has_dangling_whitespace<'a>(ctx: &Ctx<'a>, node: N<'a>) -> bool {
    let Some(children) = children_of(node) else {
        return false;
    };
    match node {
        Node::Document(_) => children.is_empty() && !ctx.source.is_empty(),
        _ => {
            if node.is_branched_tag() && children.len() == 1 {
                return has_dangling_whitespace(ctx, &children[0]);
            }
            if !children.is_empty() {
                return false;
            }
            let Some(p) = block_start_position(node) else {
                return false;
            };
            is_whitespace_at(ctx.source, p.end as usize)
        }
    }
}

fn has_leading_whitespace<'a>(ctx: &Ctx<'a>, node: N<'a>) -> bool {
    if matches!(node, Node::LiquidBranch(_)) && ctx.prev(node).is_none() {
        return match ctx.first_child(node) {
            Some(f) => has_leading_whitespace(ctx, f),
            None => has_dangling_whitespace(ctx, node),
        };
    }
    is_whitespace_before(ctx.source, node.position().start as usize)
}

fn has_trailing_whitespace<'a>(ctx: &Ctx<'a>, node: N<'a>) -> bool {
    if matches!(node, Node::LiquidBranch(_)) || is_html_element_without_close_tag(node) {
        return match ctx.last_child(node) {
            Some(l) => has_trailing_whitespace(ctx, l),
            None => has_dangling_whitespace(ctx, node),
        };
    }
    is_whitespace_at(ctx.source, node.position().end as usize)
}

fn augment_with_whitespace_helpers<'a>(ctx: &Ctx<'a>, node: N<'a>) -> u16 {
    let mut flags = 0;
    let mut set = |flag: u16, v: bool| {
        if v {
            flags |= flag;
        }
    };
    set(IS_DANGLING_WS_SENSITIVE, is_dangling_whitespace_sensitive_node(ctx, node));
    set(IS_INDENTATION_SENSITIVE, is_indentation_sensitive_node(node));
    set(IS_WHITESPACE_SENSITIVE, is_indentation_sensitive_node(node));
    let prev = ctx.prev(node);
    set(
        IS_LEADING_WS_SENSITIVE,
        is_leading_whitespace_sensitive_node(ctx, Some(node))
            && prev.is_none_or(|p| is_trailing_whitespace_sensitive_node(ctx, p)),
    );
    let next = ctx.next(node);
    set(
        IS_TRAILING_WS_SENSITIVE,
        is_trailing_whitespace_sensitive_node(ctx, node)
            && next.is_none_or(|n| is_leading_whitespace_sensitive_node(ctx, Some(n))),
    );
    set(HAS_LEADING_WS, has_leading_whitespace(ctx, node));
    set(HAS_TRAILING_WS, has_trailing_whitespace(ctx, node));
    set(HAS_DANGLING_WS, has_dangling_whitespace(ctx, node));
    flags
}

/// `Object.prototype` members: looking these up in the plugin's tag tables yields a
/// function instead of a string, and `.startsWith` on it throws. `__proto__` yields an
/// object but cannot be reached: the parser does not read it as an element name.
fn is_object_prototype_member(name: &str) -> bool {
    matches!(
        name,
        "constructor"
            | "__defineGetter__"
            | "__defineSetter__"
            | "hasOwnProperty"
            | "__lookupGetter__"
            | "__lookupSetter__"
            | "isPrototypeOf"
            | "propertyIsEnumerable"
            | "toString"
            | "valueOf"
            | "toLocaleString"
    )
}

/// The value of `cssDisplay` / `cssWhitespace` for such an element.
pub const FUNCTION_VALUE: &str = "\u{0}function";

thread_local! {
    static FIRST_TYPE_ERROR: Cell<Option<&'static str>> = const { Cell::new(None) };
}

/// Records the error upstream would throw here. Evaluation carries on, so the first one
/// recorded is the one upstream reports.
pub fn note_type_error(message: &'static str) {
    FIRST_TYPE_ERROR.with(|e| {
        if e.get().is_none() {
            e.set(Some(message));
        }
    });
}

fn post_order<'a>(node: N<'a>, out: &mut Vec<N<'a>>) {
    let mut children: Vec<N<'a>> = Vec::new();
    node.for_each_child(|c| children.push(c));
    for c in children {
        post_order(c, out);
    }
    out.push(node);
}

/// `preprocess`.
pub fn preprocess<'a>(ctx: &mut Ctx<'a>, root: N<'a>) -> Result<(), String> {
    let mut visited = Vec::with_capacity(ctx.nodes.len());
    augment_with_family(ctx, root, &mut visited);

    let mut has_function_value = false;
    for &node in &visited {
        let display = get_css_display(ctx, node);
        let whitespace = get_css_white_space(ctx, node);
        let info = &mut ctx.info[node.id().0 as usize];
        info.css_display = display;
        info.css_whitespace = whitespace;
        has_function_value |= display == FUNCTION_VALUE || whitespace == FUNCTION_VALUE;
    }

    if has_function_value {
        // Only here does evaluation order show: replay the pass in `walk`'s post-order.
        FIRST_TYPE_ERROR.with(|e| e.set(None));
        let mut order = Vec::with_capacity(visited.len());
        post_order(root, &mut order);
        for &node in &order {
            if ctx.info[node.id().0 as usize].css_display != "should not be relevant" {
                augment_with_whitespace_helpers(ctx, node);
            }
            if let Some(message) = FIRST_TYPE_ERROR.with(Cell::get) {
                return Err(message.to_string());
            }
        }
    }

    for &node in &visited {
        if ctx.info[node.id().0 as usize].css_display == "should not be relevant" {
            continue;
        }
        let flags = augment_with_whitespace_helpers(ctx, node);
        ctx.info[node.id().0 as usize].flags |= flags;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::css_from_comment;

    #[test]
    fn css_comments() {
        assert_eq!(css_from_comment(" display: block ", false), Some("block"));
        assert_eq!(css_from_comment("display:inline-block", false), None);
        assert_eq!(css_from_comment("whitespace: pre", true), Some("pre"));
        assert_eq!(css_from_comment("white-space: normal", true), Some("normal"));
        assert_eq!(css_from_comment("display: Block", false), None);
    }
}
