//! `disabled-checks/index.ts`: `{% # theme-check-disable %}` comments.

use crate::run::Offense;
use crate::visitor::find_current_node;
use lq_syntax::js::trim;
use lq_syntax::{Markup, Node, Position};

const ALL: &str = "@all";

#[derive(Clone, Copy, Debug)]
struct Range {
    from: u32,
    to: Option<u32>,
}

/// The ranges of one file. Offsets in bytes.
#[derive(Debug, Default)]
pub struct DisabledChecks {
    /// `Map<CheckName, Range[]>` in insertion order.
    ranges: Vec<(String, Vec<Range>)>,
}

impl DisabledChecks {
    fn entry(&mut self, check: &str) -> &mut Vec<Range> {
        let i = match self.ranges.iter().position(|(c, _)| c == check) {
            Some(i) => i,
            None => {
                self.ranges.push((check.to_string(), Vec::new()));
                self.ranges.len() - 1
            }
        };
        &mut self.ranges[i].1
    }

    /// The `LiquidRawTag` and `LiquidTag` handlers of `DisabledChecksVisitor`.
    #[inline]
    pub fn visit<'a>(&mut self, node: &'a Node<'a>, root: &'a Node<'a>) {
        match node {
            Node::LiquidRawTag(n) if n.name == "comment" => {
                if let Node::RawMarkup(body) = &n.body {
                    self.determine_ranges(body.value, node, root);
                }
            }
            Node::LiquidTag(n) if n.name == "#" => {
                if let Markup::Str(value) = n.markup {
                    self.determine_ranges(value, node, root);
                }
            }
            _ => {}
        }
    }

    fn determine_ranges<'a>(&mut self, value: &str, node: &'a Node<'a>, root: &'a Node<'a>) {
        // /^(?:theme\-check\-(disable-next-line|disable|enable)) ?(.*)/
        let Some(rest) = trim(value).strip_prefix("theme-check-") else { return };
        let (command, rest) = if let Some(r) = rest.strip_prefix("disable-next-line") {
            ("disable-next-line", r)
        } else if let Some(r) = rest.strip_prefix("disable") {
            ("disable", r)
        } else if let Some(r) = rest.strip_prefix("enable") {
            ("enable", r)
        } else {
            return;
        };
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        let end = rest.find(['\n', '\r', '\u{2028}', '\u{2029}']).unwrap_or(rest.len());
        let joined = &rest[..end];
        let checks: Vec<&str> = if joined.is_empty() { vec![ALL] } else { split_checks(joined) };
        let position = node.position();
        for check in checks {
            match command {
                "disable-next-line" => {
                    if let Some(next) = find_next_line_position(root, node) {
                        self.entry(check).push(Range { from: next.start, to: Some(next.end) });
                    }
                }
                "disable" => self.entry(check).push(Range { from: position.end, to: None }),
                _ => {
                    if let Some(i) = self.ranges.iter().position(|(c, _)| c == check) {
                        if let Some(last) = self.ranges[i].1.last_mut() {
                            last.to = Some(position.start);
                        }
                    } else if check == ALL {
                        for (_, ranges) in &mut self.ranges {
                            for r in ranges {
                                if r.to.is_none_or(|t| t == 0) {
                                    r.to = Some(position.start);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// `isDisabled(offense)`.
    pub fn is_disabled(&self, offense: &Offense) -> bool {
        [ALL, offense.check].iter().any(|check| {
            self.ranges.iter().filter(|(c, _)| c == check).any(|(_, ranges)| {
                ranges.iter().any(|r| {
                    offense.start_byte >= r.from
                        && (r.to.is_none_or(|t| t == 0) || r.to.is_some_and(|t| offense.end_byte <= t))
                })
            })
        })
    }
}

/// `split(/,[ ]*/)`.
fn split_checks(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find(',') {
        out.push(&rest[..i]);
        rest = rest[i + 1..].trim_start_matches(' ');
    }
    out.push(rest);
    out
}

fn same(a: &Node<'_>, b: &Node<'_>) -> bool {
    std::ptr::eq(a, b)
}

fn next_node<'a>(parent: Option<&'a Node<'a>>, node: &'a Node<'a>) -> Option<&'a Node<'a>> {
    let siblings: &'a [Node<'a>] = match parent? {
        Node::LiquidTag(t) if matches!(t.markup, Markup::List(_)) => match &t.markup {
            Markup::List(l) => l,
            _ => &[],
        },
        Node::LiquidTag(t) => t.children.as_deref().unwrap_or(&[]),
        Node::Document(n) => &n.children,
        Node::LiquidBranch(n) => &n.children,
        Node::HtmlElement(n) => &n.children,
        _ => &[],
    };
    let i = siblings.iter().position(|c| same(c, node))?;
    siblings.get(i + 1)
}

/// `findNextLinePosition(ast, node)`.
pub fn find_next_line_position<'a>(ast: &'a Node<'a>, node: &'a Node<'a>) -> Option<Position> {
    let (current, ancestors) = find_current_node(ast, node.position().end);
    let parent = ancestors.last().copied();
    let grandparent = ancestors.len().checked_sub(2).map(|i| ancestors[i]);
    let next = match next_node(parent, current) {
        Some(n) => n,
        None => next_node(grandparent, parent?)?,
    };
    Some(match next {
        Node::LiquidRawTag(n) => {
            if n.name == "doc" {
                next.position()
            } else {
                n.block_start_position
            }
        }
        Node::LiquidTag(n) => n.block_start_position,
        Node::LiquidBranch(n) => n.block_start_position,
        Node::HtmlElement(n) => n.block_start_position,
        Node::HtmlVoidElement(n) => n.block_start_position,
        Node::HtmlSelfClosingElement(n) => n.block_start_position,
        Node::HtmlRawNode(n) => n.block_start_position,
        Node::HtmlDanglingMarkerClose(n) => n.block_start_position,
        other => other.position(),
    })
}
