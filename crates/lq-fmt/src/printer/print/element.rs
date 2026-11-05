//! Port of `printer/print/element.ts`.

use crate::concat;
use crate::doc::utils::replace_end_of_line;
use crate::doc::{dedent_to_root, group, group_id, indent, Doc};
use crate::printer::string::js_trim;
use crate::printer::utils::*;
use crate::printer::*;
use lq_syntax::Node;

impl<'a> Ctx<'a> {
    fn print_raw_element(&self, node: N<'a>) -> Doc<'a> {
        let Node::HtmlRawNode(raw) = node else {
            return Doc::Nil;
        };
        let attr_group_id = self.symbol();
        let body = match &raw.body {
            Node::RawMarkup(b) if js_trim(b.value).is_empty() => Doc::Nil,
            b => self.print_node(b, Args::default()),
        };
        group(concat![
            self.print_opening_tag_prefix(node),
            group_id(self.print_opening_tag(node, attr_group_id), attr_group_id),
            body,
            self.print_closing_tag(node),
            self.print_closing_tag_suffix(node),
        ])
    }

    /// `printElement`.
    pub fn print_element(&self, node: N<'a>) -> Doc<'a> {
        if matches!(node, Node::HtmlRawNode(_)) {
            return self.print_raw_element(node);
        }
        let attr_group_id = self.symbol();

        if has_no_children(node) {
            return concat![
                group_id(self.print_opening_tag(node, attr_group_id), attr_group_id),
                self.print_closing_tag(node),
                self.print_closing_tag_suffix(node),
            ];
        }

        if self.should_preserve_content(node) {
            return concat![
                self.print_opening_tag_prefix(node),
                group_id(self.print_opening_tag(node, attr_group_id), attr_group_id),
                replace_end_of_line(self.get_html_node_content(node)),
                self.print_closing_tag(node),
                self.print_closing_tag_suffix(node),
            ];
        }

        let element_group_id = self.symbol();
        let Node::HtmlElement(element) = node else {
            return Doc::Nil;
        };
        let print_tag = |doc: Doc<'a>| {
            group_id(
                concat![
                    group_id(self.print_opening_tag(node, attr_group_id), attr_group_id),
                    doc,
                    self.print_closing_tag(node),
                ],
                element_group_id,
            )
        };
        let block_end = element.block_end_position;

        if element.children.is_empty() {
            return print_tag(
                if self.has_flag(node, HAS_DANGLING_WS)
                    && self.has_flag(node, IS_DANGLING_WS_SENSITIVE)
                    && block_end.end != block_end.start
                {
                    Doc::Line
                } else {
                    Doc::Nil
                },
            );
        }

        let first = &element.children[0];
        let last = &element.children[element.children.len() - 1];

        let line_before_children = if self.has_flag(first, HAS_LEADING_WS)
            && self.has_flag(first, IS_LEADING_WS_SENSITIVE)
        {
            Doc::Line
        } else if matches!(first, Node::TextNode(_))
            && self.has_flag(node, IS_WHITESPACE_SENSITIVE)
            && self.has_flag(node, IS_INDENTATION_SENSITIVE)
        {
            dedent_to_root(Doc::Softline)
        } else {
            Doc::Softline
        };

        let last_trailing =
            self.has_flag(last, HAS_TRAILING_WS) && self.has_flag(last, IS_TRAILING_WS_SENSITIVE);
        let line_after_children = if block_end.start == block_end.end {
            Doc::Nil
        } else {
            let needs_to_borrow = match self.next(node) {
                Some(n) => self.needs_to_borrow_prev_closing_tag_end_marker(n),
                None => self
                    .parent(node)
                    .is_some_and(|p| self.needs_to_borrow_last_child_closing_tag_end_marker(p)),
            };
            if needs_to_borrow {
                if last_trailing {
                    " ".into()
                } else {
                    Doc::Nil
                }
            } else if last_trailing {
                Doc::Line
            } else {
                Doc::Softline
            }
        };

        print_tag(concat![
            if self.force_break_content(node) { Doc::BreakParent } else { Doc::Nil },
            indent(concat![
                line_before_children,
                self.print_children(
                    node,
                    Args {
                        leading_space_group_id: GroupIds::one(element_group_id),
                        trailing_space_group_id: GroupIds::one(element_group_id),
                        ..Args::default()
                    },
                ),
            ]),
            line_after_children,
        ])
    }
}
