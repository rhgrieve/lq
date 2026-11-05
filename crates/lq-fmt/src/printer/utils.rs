//! Port of `printer/utils/node.ts` and `printer/utils/index.ts`.

use super::string::{has_line_break_in_range, is_js_ws, js_trim};
use super::*;
use crate::doc::{if_break, Doc};
use lq_syntax::{Markup, Node, Position};

/// `node.children`.
#[inline]
pub fn children_of<'a>(node: N<'a>) -> Option<&'a [Node<'a>]> {
    match node {
        Node::Document(n) => Some(&n.children),
        Node::LiquidTag(n) => n.children.as_deref(),
        Node::LiquidBranch(n) => Some(&n.children),
        Node::HtmlElement(n) => Some(&n.children),
        _ => None,
    }
}

/// `node.attributes`.
#[inline]
pub fn attributes_of<'a>(node: N<'a>) -> &'a [Node<'a>] {
    match node {
        Node::HtmlElement(n) => &n.attributes,
        Node::HtmlVoidElement(n) => &n.attributes,
        Node::HtmlSelfClosingElement(n) => &n.attributes,
        Node::HtmlRawNode(n) => &n.attributes,
        _ => &[],
    }
}

/// `node.blockStartPosition`.
#[inline]
pub fn block_start_position(node: N<'_>) -> Option<Position> {
    Some(match node {
        Node::LiquidRawTag(n) => n.block_start_position,
        Node::LiquidTag(n) => n.block_start_position,
        Node::LiquidBranch(n) => n.block_start_position,
        Node::HtmlElement(n) => n.block_start_position,
        Node::HtmlVoidElement(n) => n.block_start_position,
        Node::HtmlSelfClosingElement(n) => n.block_start_position,
        Node::HtmlRawNode(n) => n.block_start_position,
        Node::HtmlDanglingMarkerClose(n) => n.block_start_position,
        _ => return None,
    })
}

/// `node.blockEndPosition`.
#[inline]
pub fn block_end_position(node: N<'_>) -> Option<Position> {
    match node {
        Node::LiquidRawTag(n) => Some(n.block_end_position),
        Node::LiquidTag(n) => n.block_end_position,
        Node::LiquidBranch(n) => Some(n.block_end_position),
        Node::HtmlElement(n) => Some(n.block_end_position),
        Node::HtmlRawNode(n) => Some(n.block_end_position),
        _ => None,
    }
}

/// `node.name` when it is a string.
#[inline]
pub fn string_name<'a>(node: N<'a>) -> Option<&'a str> {
    match node {
        Node::LiquidRawTag(n) => Some(n.name),
        Node::LiquidTag(n) => Some(n.name),
        Node::LiquidBranch(n) => n.name,
        Node::HtmlVoidElement(n) => Some(n.name),
        Node::HtmlRawNode(n) => Some(n.name),
        _ => None,
    }
}

/// The text of a compound name made of one `TextNode`, on the node types that have one.
#[inline]
pub fn plain_compound_name<'a>(node: N<'a>) -> Option<&'a str> {
    let parts: &'a [Node<'a>] = match node {
        Node::HtmlElement(n) => &n.name,
        Node::HtmlSelfClosingElement(n) => &n.name,
        Node::HtmlDanglingMarkerClose(n) => &n.name,
        _ => return None,
    };
    match parts {
        [Node::TextNode(t)] => Some(&t.value),
        _ => None,
    }
}

#[inline]
pub fn is_script_like_tag(node: N<'_>) -> bool {
    matches!(node, Node::HtmlRawNode(_))
}

#[inline]
pub fn is_html_comment(node: N<'_>) -> bool {
    matches!(node, Node::HtmlComment(_))
}

#[inline]
pub fn is_self_closing(node: N<'_>) -> bool {
    matches!(node, Node::HtmlSelfClosingElement(_))
}

#[inline]
pub fn is_void_element(node: N<'_>) -> bool {
    matches!(node, Node::HtmlVoidElement(_))
}

#[inline]
pub fn is_html_element(node: N<'_>) -> bool {
    matches!(node, Node::HtmlElement(_))
}

#[inline]
pub fn is_text_like_node(node: N<'_>) -> bool {
    matches!(node, Node::TextNode(_))
}

#[inline]
pub fn is_liquid_node(node: N<'_>) -> bool {
    matches!(
        node,
        Node::LiquidTag(_)
            | Node::LiquidVariableOutput(_)
            | Node::LiquidBranch(_)
            | Node::LiquidRawTag(_)
    )
}

#[inline]
pub fn is_html_node(node: N<'_>) -> bool {
    matches!(
        node,
        Node::HtmlElement(_)
            | Node::HtmlDanglingMarkerClose(_)
            | Node::HtmlRawNode(_)
            | Node::HtmlVoidElement(_)
            | Node::HtmlSelfClosingElement(_)
    )
}

#[inline]
pub fn is_html_dangling_marker_open(node: N<'_>) -> bool {
    match node {
        Node::HtmlElement(n) => n.block_end_position.start == n.block_end_position.end,
        _ => false,
    }
}

#[inline]
pub fn has_no_children(node: N<'_>) -> bool {
    matches!(
        node,
        Node::HtmlSelfClosingElement(_)
            | Node::HtmlVoidElement(_)
            | Node::HtmlComment(_)
            | Node::HtmlDanglingMarkerClose(_)
    )
}

#[inline]
pub fn has_no_close_marker(node: N<'_>) -> bool {
    has_no_children(node) || is_html_dangling_marker_open(node)
}

/// `/^\s*prettier-ignore(?=\s|$)/m`.
fn matches_prettier_ignore(text: &str) -> bool {
    const IGNORE: &str = "prettier-ignore";
    if !text.contains(IGNORE) {
        return false;
    }
    let is_line_terminator = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
    let mut starts = vec![0];
    for (i, c) in text.char_indices() {
        if is_line_terminator(c) {
            starts.push(i + c.len_utf8());
        }
    }
    starts.into_iter().any(|start| {
        let rest = text[start..].trim_start_matches(is_js_ws);
        match rest.strip_prefix(IGNORE) {
            Some(after) => after.chars().next().is_none_or(is_js_ws),
            None => false,
        }
    })
}

pub fn is_prettier_ignore_node(node: Option<N<'_>>) -> bool {
    match node {
        Some(Node::HtmlComment(c)) => matches_prettier_ignore(c.body),
        Some(Node::LiquidTag(t)) if t.name == "#" => match &t.markup {
            Markup::Str(m) => matches_prettier_ignore(m),
            _ => false,
        },
        _ => false,
    }
}

/// `value.trim().match(/prettier-ignore-attribute(?:s?)(?:\s+(.+))?$/s)`.
fn get_prettier_ignore_attribute_comment_data(value: &str) -> bool {
    const IGNORE: &str = "prettier-ignore-attribute";
    let value = js_trim(value);
    let mut from = 0;
    while let Some(i) = value[from..].find(IGNORE) {
        let after = &value[from + i + IGNORE.len()..];
        for rest in [Some(after), after.strip_prefix('s')].into_iter().flatten() {
            if rest.is_empty() {
                return true;
            }
            let mut chars = rest.chars();
            if chars.next().is_some_and(is_js_ws) && chars.next().is_some() {
                return true;
            }
        }
        from += i + 1;
    }
    false
}

pub fn is_prettier_ignore_attribute_node(node: Option<N<'_>>) -> bool {
    match node {
        Some(Node::HtmlComment(c)) => get_prettier_ignore_attribute_comment_data(c.body),
        Some(Node::LiquidTag(t)) if t.name == "#" => match &t.markup {
            Markup::Str(m) => get_prettier_ignore_attribute_comment_data(m),
            _ => false,
        },
        _ => false,
    }
}

fn is_tag_name_included(collection: &[&str], node: N<'_>) -> bool {
    plain_compound_name(node).is_some_and(|n| collection.contains(&n))
}

fn has_non_text_child(node: N<'_>) -> bool {
    children_of(node).is_some_and(|c| c.iter().any(|c| !is_text_like_node(c)))
}

/// `Array.prototype.includes`-free helper for `isEmpty(node.children || [])`.
fn has_children(node: N<'_>) -> bool {
    children_of(node).is_some_and(|c| !c.is_empty())
}

/// `isDeeplyNested`.
pub fn is_deeply_nested(node: N<'_>) -> bool {
    let Some(children) = children_of(node) else {
        return false;
    };
    if node.is_branched_tag() {
        return children.iter().any(is_deeply_nested);
    }
    children.iter().any(has_children)
}

impl<'a> Ctx<'a> {
    #[inline]
    pub fn is_pre_like_node(&self, node: N<'a>) -> bool {
        let whitespace = self.get(node).css_whitespace;
        if whitespace == super::preprocess::FUNCTION_VALUE {
            super::preprocess::note_type_error("node.cssWhitespace.startsWith is not a function");
        }
        whitespace.starts_with("pre")
    }

    #[inline]
    pub fn should_preserve_content(&self, node: N<'a>) -> bool {
        self.is_pre_like_node(node)
    }

    #[inline]
    pub fn is_attribute_node(&self, node: N<'a>) -> bool {
        self.has_flag(node, IS_ATTRIBUTE)
    }

    pub fn has_prettier_ignore(&self, node: N<'a>) -> bool {
        is_prettier_ignore_node(Some(node)) || is_prettier_ignore_node(self.prev(node))
    }

    /// `forceNextEmptyLine`.
    pub fn force_next_empty_line(&self, node: Option<N<'a>>) -> bool {
        let Some(node) = node else {
            return false;
        };
        let Some(next) = self.next(node) else {
            return false;
        };
        let bytes = self.source.as_bytes();
        let start = node.position().end as usize;
        if start >= bytes.len() {
            return false;
        }
        let mut iter = memchr::memchr_iter(b'\n', &bytes[start..]);
        if iter.next().is_none() {
            return false;
        }
        match iter.next() {
            Some(i) => start + i < next.position().start as usize,
            None => false,
        }
    }

    /// `forceBreakChildren`.
    pub fn force_break_children(&self, node: N<'a>) -> bool {
        match node {
            Node::HtmlElement(n) if !n.children.is_empty() => {
                let display = self.get(node).css_display;
                is_tag_name_included(&["html", "head", "ul", "ol", "select"], node)
                    || (display.starts_with("table") && display != "table-cell")
            }
            _ => false,
        }
    }

    /// `forceBreakContent`.
    pub fn force_break_content(&self, node: N<'a>) -> bool {
        if self.force_break_children(node) {
            return true;
        }
        if let Node::HtmlElement(n) = node {
            if !n.children.is_empty()
                && (is_tag_name_included(&["body", "script", "style"], node)
                    || n.children.iter().any(has_non_text_child))
            {
                return true;
            }
        }
        let (Some(first), Some(last)) = (self.first_child(node), self.last_child(node)) else {
            return false;
        };
        Ctx::same(first, last)
            && !is_text_like_node(first)
            && self.has_leading_line_break(first)
            && (!self.has_flag(last, IS_TRAILING_WS_SENSITIVE) || self.has_trailing_line_break(last))
    }

    fn prefer_hardline_as_surrounding_spaces(&self, node: N<'a>) -> bool {
        match node {
            Node::HtmlComment(_) => true,
            Node::HtmlElement(_) => is_tag_name_included(&["script", "select"], node),
            Node::LiquidTag(t) => {
                if self.prev(node).is_some_and(is_text_like_node)
                    || self.next(node).is_some_and(is_text_like_node)
                {
                    return false;
                }
                t.children.as_ref().is_some_and(|c| !c.is_empty())
            }
            _ => false,
        }
    }

    /// `preferHardlineAsLeadingSpaces`.
    pub fn prefer_hardline_as_leading_spaces(&self, node: N<'a>) -> bool {
        self.prefer_hardline_as_surrounding_spaces(node)
            || (is_liquid_node(node) && self.prev(node).is_some_and(is_liquid_node))
            || self.prev(node).is_some_and(|p| self.prefer_hardline_as_trailing_spaces(p))
            || self.has_surrounding_line_break(node)
    }

    fn prefer_hardline_as_trailing_spaces(&self, node: N<'a>) -> bool {
        self.prefer_hardline_as_surrounding_spaces(node)
            || (is_liquid_node(node)
                && self.next(node).is_some_and(|n| is_liquid_node(n) || is_html_node(n)))
            || (is_html_element(node) && is_tag_name_included(&["br"], node))
            || self.has_surrounding_line_break(node)
    }

    #[inline]
    pub fn has_meaningful_lack_of_leading_whitespace(&self, node: N<'a>) -> bool {
        let f = self.get(node).flags;
        f & IS_LEADING_WS_SENSITIVE != 0 && f & HAS_LEADING_WS == 0
    }

    #[inline]
    pub fn has_meaningful_lack_of_trailing_whitespace(&self, node: N<'a>) -> bool {
        let f = self.get(node).flags;
        f & IS_TRAILING_WS_SENSITIVE != 0 && f & HAS_TRAILING_WS == 0
    }

    #[inline]
    pub fn has_meaningful_lack_of_dangling_whitespace(&self, node: N<'a>) -> bool {
        let f = self.get(node).flags;
        f & IS_DANGLING_WS_SENSITIVE != 0 && f & HAS_DANGLING_WS == 0
    }

    fn has_surrounding_line_break(&self, node: N<'a>) -> bool {
        self.has_leading_line_break(node) && self.has_trailing_line_break(node)
    }

    fn has_leading_line_break(&self, node: N<'a>) -> bool {
        if matches!(node, Node::Document(_)) || !self.has_flag(node, HAS_LEADING_WS) {
            return false;
        }
        let start = match self.prev(node) {
            Some(p) => p.position().end,
            None => {
                let Some(parent) = self.parent(node) else {
                    return false;
                };
                match block_start_position(parent) {
                    Some(b) => b.end,
                    None => parent.position().start,
                }
            }
        };
        has_line_break_in_range(self.source, start as usize, node.position().start as usize)
    }

    fn has_trailing_line_break(&self, node: N<'a>) -> bool {
        if matches!(node, Node::Document(_)) || !self.has_flag(node, HAS_TRAILING_WS) {
            return false;
        }
        let end = match self.next(node) {
            Some(n) => n.position().start,
            None => {
                let Some(parent) = self.parent(node) else {
                    return false;
                };
                match block_end_position(parent) {
                    Some(b) => b.start,
                    None => parent.position().end,
                }
            }
        };
        has_line_break_in_range(self.source, node.position().end as usize, end as usize)
    }

    /// `getLastDescendant`.
    pub fn get_last_descendant(&self, node: N<'a>) -> N<'a> {
        let mut node = node;
        while let Some(l) = self.last_child(node) {
            node = l;
        }
        node
    }
}

/// `getWhitespaceTrim`.
pub fn get_whitespace_trim<'a>(
    curr_whitespace_trim: &'static str,
    needs_whitespace_stripping_on_break: bool,
    group_ids: GroupIds,
) -> Doc<'a> {
    let breaks = if needs_whitespace_stripping_on_break { "-" } else { curr_whitespace_trim };
    if_break_chain(breaks, curr_whitespace_trim, group_ids)
}

/// `ifBreakChain`.
pub fn if_break_chain<'a>(
    breaks_content: &'static str,
    flat_content: &'static str,
    group_ids: GroupIds,
) -> Doc<'a> {
    let ids = group_ids.as_slice();
    if ids.contains(&FORCE_BREAK_GROUP_ID) {
        return Doc::from(breaks_content);
    }
    if ids.contains(&FORCE_FLAT_GROUP_ID) {
        return Doc::from(flat_content);
    }
    if breaks_content == flat_content {
        // Every branch of the chain prints the same string.
        return Doc::from(flat_content);
    }
    let mut curr = Doc::from(flat_content);
    for &id in ids {
        curr = if_break(breaks_content, curr, id);
    }
    curr
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prettier_ignore_regex() {
        assert!(matches_prettier_ignore(" prettier-ignore "));
        assert!(matches_prettier_ignore("prettier-ignore"));
        assert!(matches_prettier_ignore("foo\n  prettier-ignore\nbar"));
        assert!(!matches_prettier_ignore("prettier-ignored"));
        assert!(!matches_prettier_ignore("foo prettier-ignore"));
        assert!(!matches_prettier_ignore("prettier-ignore-attribute"));
    }

    #[test]
    fn prettier_ignore_attribute_regex() {
        let f = get_prettier_ignore_attribute_comment_data;
        assert!(f("prettier-ignore-attribute"));
        assert!(f("prettier-ignore-attributes"));
        assert!(f(" prettier-ignore-attribute a b "));
        assert!(f("xx prettier-ignore-attribute"));
        assert!(!f("prettier-ignore-attributesx"));
        assert!(!f("prettier-ignore"));
    }
}
