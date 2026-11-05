//! Port of `printer/print/tag.ts`.

use crate::concat;
use crate::doc::utils::replace_end_of_line;
use crate::doc::{indent, join_with, Doc};
use crate::options::Options;
use crate::printer::string::{
    has_more_than_one_newline_between, js_trim, slice,
};
use crate::printer::utils::*;
use crate::printer::*;
use lq_syntax::{Markup, Node};
use std::borrow::Cow;

const NO: Cow<'static, str> = Cow::Borrowed("");

impl<'a> Ctx<'a> {
    fn should_not_print_closing_tag(&self, node: N<'a>) -> bool {
        !has_no_close_marker(node)
            && block_end_position(node).is_none()
            && (self.has_prettier_ignore(node)
                || self.parent(node).is_some_and(|p| self.should_preserve_content(p)))
    }

    pub fn needs_to_borrow_prev_closing_tag_end_marker(&self, node: N<'a>) -> bool {
        !is_liquid_node(node)
            && self.prev(node).is_some_and(is_html_node)
            && self.has_meaningful_lack_of_leading_whitespace(node)
    }

    pub fn needs_to_borrow_last_child_closing_tag_end_marker(&self, node: N<'a>) -> bool {
        is_html_node(node)
            && self.last_child(node).is_some_and(|l| {
                self.has_meaningful_lack_of_trailing_whitespace(l)
                    && is_html_node(self.get_last_descendant(l))
            })
            && !self.is_pre_like_node(node)
    }

    pub fn needs_to_borrow_parent_closing_tag_start_marker(&self, node: N<'a>) -> bool {
        self.parent(node).is_some_and(is_html_node)
            && self.next(node).is_none()
            && self.has_meaningful_lack_of_trailing_whitespace(node)
            && !is_liquid_node(node)
            && {
                let d = self.get_last_descendant(node);
                is_text_like_node(d) || is_liquid_node(d)
            }
    }

    pub fn needs_to_borrow_next_opening_tag_start_marker(&self, node: N<'a>) -> bool {
        self.next(node).is_some_and(is_html_node)
            && is_text_like_node(node)
            && self.has_meaningful_lack_of_trailing_whitespace(node)
    }

    pub fn needs_to_borrow_parent_opening_tag_end_marker(&self, node: N<'a>) -> bool {
        self.parent(node).is_some_and(is_html_node)
            && self.prev(node).is_none()
            && self.has_meaningful_lack_of_leading_whitespace(node)
            && !is_liquid_node(node)
    }

    fn print_attributes(&self, node: N<'a>, attr_group_id: GroupId) -> Doc<'a> {
        if is_html_comment(node) || matches!(node, Node::HtmlDanglingMarkerClose(_)) {
            return Doc::Nil;
        }
        let attributes = attributes_of(node);
        let options: &Options = self.options;
        if attributes.is_empty() {
            return if is_self_closing(node) { " ".into() } else { Doc::Nil };
        }

        let prettier_ignore_attributes = is_prettier_ignore_attribute_node(self.prev(node));

        let printed_attributes: Vec<Doc<'a>> = attributes
            .iter()
            .map(|attr| {
                let extra_newline = match self.prev(attr) {
                    Some(p)
                        if has_more_than_one_newline_between(
                            self.source,
                            p.position().end as usize,
                            attr.position().start as usize,
                        ) =>
                    {
                        Doc::Hardline
                    }
                    _ => Doc::Nil,
                };
                let printed = self.print_node(
                    attr,
                    Args {
                        trailing_space_group_id: GroupIds::one(attr_group_id),
                        ..Args::default()
                    },
                );
                concat![extra_newline, printed]
            })
            .collect();

        let last_end = attributes[attributes.len() - 1].position().end as usize;
        let block_start = block_start_position(node).map_or(0, |p| p.start as usize);
        let force_break_attr_content = slice(self.source, block_start, last_end).contains('\n');

        let is_single_line_link_tag_exception =
            options.single_line_link_tags && string_name(node) == Some("link");

        let should_not_break_attributes = ((is_html_element(node)
            && children_of(node).is_some_and(|c| !c.is_empty()))
            || is_void_element(node)
            || is_self_closing(node))
            && !force_break_attr_content
            && attributes.len() == 1
            && !is_liquid_node(&attributes[0]);

        let force_not_to_break_attr_content =
            is_single_line_link_tag_exception || should_not_break_attributes;

        let attributes_doc = if prettier_ignore_attributes {
            replace_end_of_line(slice(
                self.source,
                attributes[0].position().start as usize,
                last_end,
            ))
        } else {
            join_with(printed_attributes, || {
                if force_not_to_break_attr_content {
                    " ".into()
                } else if options.single_attribute_per_line && attributes.len() > 1 {
                    Doc::Hardline
                } else {
                    Doc::Line
                }
            })
        };

        let trailing_inner_whitespace = if self
            .first_child(node)
            .is_some_and(|f| self.needs_to_borrow_parent_opening_tag_end_marker(f))
            || (has_no_close_marker(node)
                && self
                    .parent(node)
                    .is_some_and(|p| self.needs_to_borrow_last_child_closing_tag_end_marker(p)))
            || force_not_to_break_attr_content
            || options.bracket_same_line
        {
            if is_self_closing(node) {
                " ".into()
            } else {
                Doc::Nil
            }
        } else if is_self_closing(node) {
            Doc::Line
        } else {
            Doc::Softline
        };

        concat![
            indent(concat![
                if force_not_to_break_attr_content { Doc::Str(" ") } else { Doc::Line },
                if force_break_attr_content { Doc::BreakParent } else { Doc::Nil },
                attributes_doc,
            ]),
            trailing_inner_whitespace,
        ]
    }

    /// `printOpeningTag`.
    pub fn print_opening_tag(&self, node: N<'a>, attr_group_id: GroupId) -> Doc<'a> {
        concat![
            self.print_opening_tag_start(node),
            self.print_attributes(node, attr_group_id),
            if has_no_children(node) { Doc::Nil } else { self.print_opening_tag_end(node) },
        ]
    }

    fn print_opening_tag_start(&self, node: N<'a>) -> Doc<'a> {
        if self.prev(node).is_some_and(|p| self.needs_to_borrow_next_opening_tag_start_marker(p)) {
            Doc::Nil
        } else {
            concat![self.print_opening_tag_prefix(node), self.print_opening_tag_start_marker(Some(node))]
        }
    }

    /// `printOpeningTagPrefix`.
    pub fn print_opening_tag_prefix(&self, node: N<'a>) -> Cow<'a, str> {
        if self.needs_to_borrow_parent_opening_tag_end_marker(node) {
            self.print_opening_tag_end_marker(self.parent(node))
        } else if self.needs_to_borrow_prev_closing_tag_end_marker(node) {
            self.print_closing_tag_end_marker(self.prev(node))
        } else {
            NO
        }
    }

    fn print_opening_tag_end(&self, node: N<'a>) -> Doc<'a> {
        if self.first_child(node).is_some_and(|f| self.needs_to_borrow_parent_opening_tag_end_marker(f)) {
            Doc::Nil
        } else {
            self.print_opening_tag_end_marker(Some(node)).into()
        }
    }

    /// `printOpeningTagStartMarker`.
    pub fn print_opening_tag_start_marker(&self, node: Option<N<'a>>) -> Cow<'a, str> {
        let Some(node) = node else {
            return NO;
        };
        match node {
            Node::HtmlComment(_) => Cow::Borrowed("<!--"),
            Node::HtmlElement(_) | Node::HtmlSelfClosingElement(_) => {
                Cow::Owned(format!("<{}", self.get_compound_name(node)))
            }
            Node::HtmlDanglingMarkerClose(_) => {
                Cow::Owned(format!("</{}", self.get_compound_name(node)))
            }
            Node::HtmlVoidElement(n) => Cow::Owned(format!("<{}", n.name)),
            Node::HtmlRawNode(n) => Cow::Owned(format!("<{}", n.name)),
            _ => NO,
        }
    }

    /// `printClosingTag`.
    pub fn print_closing_tag(&self, node: N<'a>) -> Doc<'a> {
        concat![
            if has_no_close_marker(node) { Doc::Nil } else { self.print_closing_tag_start(node) },
            self.print_closing_tag_end(node),
        ]
    }

    fn print_closing_tag_start(&self, node: N<'a>) -> Doc<'a> {
        if self.last_child(node).is_some_and(|l| self.needs_to_borrow_parent_closing_tag_start_marker(l)) {
            Doc::Nil
        } else {
            concat![self.print_closing_tag_prefix(node), self.print_closing_tag_start_marker(Some(node))]
        }
    }

    fn print_closing_tag_end(&self, node: N<'a>) -> Doc<'a> {
        let borrowed = match self.next(node) {
            Some(n) => self.needs_to_borrow_prev_closing_tag_end_marker(n),
            None => self
                .parent(node)
                .is_some_and(|p| self.needs_to_borrow_last_child_closing_tag_end_marker(p)),
        };
        if borrowed {
            Doc::Nil
        } else {
            concat![self.print_closing_tag_end_marker(Some(node)), self.print_closing_tag_suffix(node)]
        }
    }

    fn print_closing_tag_prefix(&self, node: N<'a>) -> Cow<'a, str> {
        if self.needs_to_borrow_last_child_closing_tag_end_marker(node) {
            self.print_closing_tag_end_marker(self.last_child(node))
        } else {
            NO
        }
    }

    /// `printClosingTagSuffix`.
    pub fn print_closing_tag_suffix(&self, node: N<'a>) -> Cow<'a, str> {
        if self.needs_to_borrow_parent_closing_tag_start_marker(node) {
            self.print_closing_tag_start_marker(self.parent(node))
        } else if self.needs_to_borrow_next_opening_tag_start_marker(node) {
            self.print_opening_tag_start_marker(self.next(node))
        } else {
            NO
        }
    }

    /// `printClosingTagStartMarker`.
    pub fn print_closing_tag_start_marker(&self, node: Option<N<'a>>) -> Cow<'a, str> {
        let Some(node) = node else {
            return NO;
        };
        if self.should_not_print_closing_tag(node) {
            return NO;
        }
        match node {
            Node::HtmlElement(_) => Cow::Owned(format!("</{}", self.get_compound_name(node))),
            Node::HtmlRawNode(n) => Cow::Owned(format!("</{}", n.name)),
            _ => NO,
        }
    }

    /// `printClosingTagEndMarker`.
    pub fn print_closing_tag_end_marker(&self, node: Option<N<'a>>) -> Cow<'a, str> {
        let Some(node) = node else {
            return NO;
        };
        if self.should_not_print_closing_tag(node) || is_html_dangling_marker_open(node) {
            return NO;
        }
        match node {
            Node::HtmlSelfClosingElement(_) => Cow::Borrowed("/>"),
            _ => Cow::Borrowed(">"),
        }
    }

    /// `printOpeningTagEndMarker`.
    pub fn print_opening_tag_end_marker(&self, node: Option<N<'a>>) -> Cow<'a, str> {
        match node {
            None => NO,
            Some(Node::HtmlComment(_)) => Cow::Borrowed("-->"),
            Some(Node::HtmlSelfClosingElement(_) | Node::HtmlVoidElement(_)) => NO,
            Some(_) => Cow::Borrowed(">"),
        }
    }

    /// `getNodeContent` of tag.ts.
    pub fn get_html_node_content(&self, node: N<'a>) -> &'a str {
        let mut start = block_start_position(node).map_or(0, |p| p.end as usize);
        if self.first_child(node).is_some_and(|f| self.needs_to_borrow_parent_opening_tag_end_marker(f)) {
            start = start.saturating_sub(self.print_opening_tag_end_marker(Some(node)).len());
        }
        let mut end = block_end_position(node).map_or(0, |p| p.start as usize);
        if let Some(last) = self.last_child(node) {
            if self.needs_to_borrow_parent_closing_tag_start_marker(last) {
                end += self.print_closing_tag_start_marker(Some(node)).len();
            } else if self.needs_to_borrow_last_child_closing_tag_end_marker(node) {
                end = end.saturating_sub(self.print_closing_tag_end_marker(Some(last)).len());
            }
        }
        slice(self.source, start, end)
    }

    /// `getCompoundName`.
    fn get_compound_name(&self, node: N<'a>) -> String {
        let parts: &[Node<'a>] = match node {
            Node::HtmlElement(n) => &n.name,
            Node::HtmlSelfClosingElement(n) => &n.name,
            Node::HtmlDanglingMarkerClose(n) => &n.name,
            _ => &[],
        };
        let mut out = String::new();
        for part in parts {
            match part {
                Node::TextNode(t) => out.push_str(&t.value),
                Node::LiquidVariableOutput(o) => {
                    out.push_str("{{ ");
                    match &o.markup {
                        Markup::Str(m) => out.push_str(js_trim(m)),
                        Markup::Node(v) => {
                            if let Node::LiquidVariable(v) = &**v {
                                out.push_str(v.raw_source);
                            }
                        }
                        _ => {}
                    }
                    out.push_str(" }}");
                }
                other => {
                    let p = other.position();
                    out.push_str(slice(self.source, p.start as usize, p.end as usize));
                }
            }
        }
        out
    }
}
