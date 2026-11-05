//! Port of `printer/print/children.ts`.

use crate::concat;
use crate::doc::utils::replace_end_of_line;
use crate::doc::{group_id, if_break, Doc};
use crate::printer::string::{js_trim_end, slice};
use crate::printer::utils::*;
use crate::printer::*;
use lq_syntax::Node;

/// The result of `printBetweenLine`, whose identity upstream compares.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BetweenLine {
    None,
    Hardline,
    Line,
    Softline,
}

impl BetweenLine {
    fn doc<'a>(self) -> Doc<'a> {
        match self {
            BetweenLine::None => Doc::Nil,
            BetweenLine::Hardline => Doc::Hardline,
            BetweenLine::Line => Doc::Line,
            BetweenLine::Softline => Doc::Softline,
        }
    }
}

/// `WhitespaceBetweenNode`.
#[derive(Default)]
struct WhitespaceBetweenNode {
    /// Number of hardlines.
    leading_hardlines: u8,
    /// `Some(Line | Softline)`, or `IfBreak` over the previous trailing group.
    leading_whitespace: Option<LeadingWhitespace>,
    leading_dependent_whitespace: bool,
    trailing_whitespace: Option<BetweenLine>,
    trailing_hardlines: u8,
}

#[derive(Clone, Copy)]
enum LeadingWhitespace {
    Between(BetweenLine),
    IfBreakOverPrevious,
}

impl<'a> Ctx<'a> {
    fn print_child(&self, child: N<'a>, args: Args) -> Doc<'a> {
        if self.has_prettier_ignore(child) {
            let is_prev_borrowing_opening_marker = self
                .prev(child)
                .is_some_and(|p| self.needs_to_borrow_next_opening_tag_start_marker(p));
            let body_start_offset = if is_prev_borrowing_opening_marker {
                self.print_opening_tag_start_marker(Some(child)).len()
            } else {
                0
            };
            let body_start = child.position().start as usize + body_start_offset;

            let is_next_borrowing_closing_marker = self
                .next(child)
                .is_some_and(|n| self.needs_to_borrow_prev_closing_tag_end_marker(n));
            let body_end_offset = if is_next_borrowing_closing_marker {
                self.print_closing_tag_end_marker(Some(child)).len()
            } else {
                0
            };
            let body_end = (child.position().end as usize).saturating_sub(body_end_offset);

            let mut raw_content = slice(self.source, body_start, body_end);
            if matches!(child, Node::HtmlElement(_)) && is_next_borrowing_closing_marker {
                raw_content = js_trim_end(raw_content);
            }

            return concat![
                self.print_opening_tag_prefix(child),
                replace_end_of_line(raw_content),
                self.print_closing_tag_suffix(child),
            ];
        }
        self.print_node(child, args)
    }

    fn print_between_line(&self, prev_node: Option<N<'a>>, next_node: Option<N<'a>>) -> BetweenLine {
        let (Some(prev_node), Some(next_node)) = (prev_node, next_node) else {
            return BetweenLine::None;
        };

        let space_between_lines_is_handled_somewhere_else =
            (self.needs_to_borrow_next_opening_tag_start_marker(prev_node)
                && (self.has_prettier_ignore(next_node)
                    || self.first_child(next_node).is_some()
                    || has_no_children(next_node)
                    || (matches!(next_node, Node::HtmlElement(_))
                        && !attributes_of(next_node).is_empty())))
                || (matches!(prev_node, Node::HtmlElement(_))
                    && has_no_close_marker(prev_node)
                    && self.needs_to_borrow_prev_closing_tag_end_marker(next_node));

        if space_between_lines_is_handled_somewhere_else {
            return BetweenLine::None;
        }

        let should_use_hardline = !self.has_flag(next_node, IS_LEADING_WS_SENSITIVE)
            || self.prefer_hardline_as_leading_spaces(next_node)
            || (self.needs_to_borrow_prev_closing_tag_end_marker(next_node)
                && self.last_child(prev_node).is_some_and(|l| {
                    self.needs_to_borrow_parent_closing_tag_start_marker(l)
                        && self
                            .last_child(l)
                            .is_some_and(|ll| self.needs_to_borrow_parent_closing_tag_start_marker(ll))
                }));

        if should_use_hardline {
            return BetweenLine::Hardline;
        }

        if self.has_flag(next_node, HAS_LEADING_WS) {
            BetweenLine::Line
        } else {
            BetweenLine::Softline
        }
    }

    /// `printChildren` for `node.children`.
    pub fn print_children(&self, node: N<'a>, args: Args) -> Doc<'a> {
        let children = children_of(node).unwrap_or(&[]);

        if self.force_break_children(node) {
            let mut parts = Vec::with_capacity(children.len() + 1);
            parts.push(Doc::BreakParent);
            for child in children {
                let prev = self.prev(child);
                let prev_between_line = self.print_between_line(prev, Some(child));
                parts.push(concat![
                    if prev_between_line == BetweenLine::None {
                        Doc::Nil
                    } else {
                        concat![
                            prev_between_line.doc(),
                            if self.force_next_empty_line(prev) { Doc::Hardline } else { Doc::Nil },
                        ]
                    },
                    self.print_child(
                        child,
                        Args {
                            leading_space_group_id: GroupIds::one(FORCE_BREAK_GROUP_ID),
                            trailing_space_group_id: GroupIds::one(FORCE_BREAK_GROUP_ID),
                            ..args
                        },
                    ),
                ]);
            }
            return Doc::Concat(parts);
        }

        let leading_space_group_ids: Vec<GroupId> = children.iter().map(|_| self.symbol()).collect();
        let trailing_space_group_ids: Vec<GroupId> = children.iter().map(|_| self.symbol()).collect();

        let whitespace_between_node: Vec<WhitespaceBetweenNode> =
            children.iter().map(|child| self.whitespace_between_node(child)).collect();

        let leading_space_group_id = |index: usize| -> GroupIds {
            if index == 0 {
                return args.leading_space_group_id;
            }
            let prev = &whitespace_between_node[index - 1];
            let curr = &whitespace_between_node[index];
            if prev.trailing_hardlines > 0 || curr.leading_hardlines > 0 {
                return GroupIds::one(FORCE_BREAK_GROUP_ID);
            }
            let mut ids = [0; 3];
            let mut n = 0;
            if prev.trailing_whitespace.is_some() {
                ids[n] = trailing_space_group_ids[index - 1];
                n += 1;
            }
            if curr.leading_whitespace.is_some() {
                ids[n] = leading_space_group_ids[index];
                n += 1;
            }
            if curr.leading_dependent_whitespace {
                ids[n] = trailing_space_group_ids[index];
                n += 1;
            }
            if n == 0 {
                ids[0] = FORCE_FLAT_GROUP_ID;
                n = 1;
            }
            GroupIds::from_vec(&ids[..n])
        };

        let trailing_space_group_id = |index: usize| -> GroupIds {
            if index == whitespace_between_node.len() - 1 {
                return args.trailing_space_group_id;
            }
            let curr = &whitespace_between_node[index];
            let next = &whitespace_between_node[index + 1];
            if curr.trailing_hardlines > 0 || next.leading_hardlines > 0 {
                return GroupIds::one(FORCE_BREAK_GROUP_ID);
            }
            if curr.trailing_whitespace.is_some() {
                return GroupIds::one(trailing_space_group_ids[index]);
            }
            GroupIds::one(FORCE_FLAT_GROUP_ID)
        };

        let mut parts = Vec::with_capacity(children.len());
        for (child_index, child) in children.iter().enumerate() {
            let w = &whitespace_between_node[child_index];
            let mut item = Vec::with_capacity(4);
            for _ in 0..w.leading_hardlines {
                item.push(Doc::Hardline);
            }

            let mut inner = Vec::with_capacity(3);
            if w.leading_dependent_whitespace {
                inner.push(Doc::Softline);
            }
            inner.push(self.print_child(
                child,
                Args {
                    leading_space_group_id: leading_space_group_id(child_index),
                    trailing_space_group_id: trailing_space_group_id(child_index),
                    ..args
                },
            ));
            if let Some(t) = w.trailing_whitespace {
                inner.push(t.doc());
            }

            let mut outer = Vec::with_capacity(2);
            match w.leading_whitespace {
                Some(LeadingWhitespace::Between(b)) => outer.push(b.doc()),
                Some(LeadingWhitespace::IfBreakOverPrevious) => outer.push(if_break(
                    Doc::Nil,
                    Doc::Softline,
                    trailing_space_group_ids[child_index - 1],
                )),
                None => {}
            }
            outer.push(group_id(inner, trailing_space_group_ids[child_index]));

            item.push(group_id(outer, leading_space_group_ids[child_index]));
            for _ in 0..w.trailing_hardlines {
                item.push(Doc::Hardline);
            }
            parts.push(Doc::Concat(item));
        }
        Doc::Concat(parts)
    }

    fn whitespace_between_node(&self, child: N<'a>) -> WhitespaceBetweenNode {
        let mut w = WhitespaceBetweenNode::default();
        let prev = self.prev(child);
        let next = self.next(child);
        let prev_between_line = self.print_between_line(prev, Some(child));
        let next_between_line = self.print_between_line(Some(child), next);

        if is_text_like_node(child) {
            return w;
        }

        if prev_between_line != BetweenLine::None {
            if self.force_next_empty_line(prev) {
                w.leading_hardlines = 2;
            } else if prev_between_line == BetweenLine::Hardline {
                w.leading_hardlines = 1;
            } else if prev.is_some_and(is_text_like_node) {
                if is_liquid_node(child) && prev_between_line == BetweenLine::Softline {
                    w.leading_dependent_whitespace = true;
                } else {
                    w.leading_whitespace = Some(LeadingWhitespace::Between(prev_between_line));
                }
            } else {
                w.leading_whitespace = Some(LeadingWhitespace::IfBreakOverPrevious);
            }
        }

        if next_between_line != BetweenLine::None {
            if self.force_next_empty_line(Some(child)) {
                if next.is_some_and(is_text_like_node) {
                    w.trailing_hardlines = 2;
                }
            } else if next_between_line == BetweenLine::Hardline {
                if next.is_some_and(is_text_like_node) {
                    w.trailing_hardlines = 1;
                }
            } else {
                w.trailing_whitespace = Some(next_between_line);
            }
        }
        w
    }
}
