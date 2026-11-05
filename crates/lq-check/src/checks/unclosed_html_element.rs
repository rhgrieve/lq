//! `UnclosedHTMLElement`: unbalanced HTML across the branches of Liquid conditions.
//!
//! Unclosed nodes must share a parent condition and a grandparent node, so they
//! are grouped by those; what's left once each group is balanced is reported.

use crate::api::*;
use lq_json::validator::JsError;
use lq_syntax::{LiteralValue, Markup, Node};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::HtmlElement, NodeKind::HtmlDanglingMarkerClose]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::<UnclosedHtmlElement<'a>>::default())
}

/// `Stacks`: its `Map`s by condition identifier in one list, in `Set` insertion order.
struct Group<'a> {
    identifier: String,
    open: Vec<&'a Node<'a>>,
    close: Vec<&'a Node<'a>>,
}

#[derive(Default)]
struct UnclosedHtmlElement<'a> {
    /// `stacksByGrandparent`, in `Map` insertion order. Keys are by node identity.
    stacks: Vec<(&'a Node<'a>, Vec<Group<'a>>)>,
}

impl<'a> LiquidCheck<'a> for UnclosedHtmlElement<'a> {
    fn enter(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let is_open = match node {
            Node::HtmlElement(e) => {
                if e.block_end_position.start != e.block_end_position.end {
                    return;
                }
                true
            }
            Node::HtmlDanglingMarkerClose(_) => false,
            _ => return,
        };
        let Some(index) = ancestors.iter().rposition(|a| matches!(a, Node::LiquidBranch(_))) else {
            return;
        };
        if index < 2 {
            return;
        }
        let Node::LiquidBranch(branch) = ancestors[index] else { return };
        let Node::LiquidTag(parent) = ancestors[index - 1] else { return };
        let grandparent = ancestors[index - 2];

        let stack = match self.stacks.iter().position(|(g, _)| std::ptr::eq(*g, grandparent)) {
            Some(i) => i,
            None => {
                self.stacks.push((grandparent, Vec::new()));
                self.stacks.len() - 1
            }
        };
        let fixed = cx.divergence(Divergence::UnclosedHtmlIdentifier);
        let identifier = match condition_identifier(fixed, branch, parent) {
            Ok(identifier) => identifier,
            Err(error) => return cx.fail(error),
        };
        let groups = &mut self.stacks[stack].1;
        let group = match groups.iter().position(|g| g.identifier == identifier) {
            Some(i) => i,
            None => {
                groups.push(Group { identifier, open: Vec::new(), close: Vec::new() });
                groups.len() - 1
            }
        };
        if is_open {
            groups[group].open.push(node);
        } else {
            groups[group].close.push(node);
        }
    }

    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {
        let source = cx.source();
        for (grandparent, groups) in &self.stacks {
            // Match opens and closes within each condition identifier.
            let unmatched: Vec<(&str, Vec<&'a Node<'a>>)> = groups
                .iter()
                .map(|g| {
                    let mut nodes: Vec<&'a Node<'a>> = Vec::with_capacity(g.open.len() + g.close.len());
                    nodes.extend(&g.open);
                    nodes.extend(&g.close);
                    nodes.sort_by_key(|n| n.position().start);
                    (g.identifier.as_str(), balance(nodes, source))
                })
                .collect();

            // Then an open in one condition may match a close in a sibling condition
            // (`{% if forloop.first %}` and `{% if forloop.last %}`).
            let mut all: Vec<&'a Node<'a>> = unmatched.iter().flat_map(|(_, n)| n.iter().copied()).collect();
            all.sort_by_key(|n| n.position().start);
            let remaining = balance(all, source);

            for (identifier, nodes) in &unmatched {
                for node in nodes {
                    if !remaining.iter().any(|r| std::ptr::eq(*r, *node)) {
                        continue;
                    }
                    let in_ = grandparent.node_type();
                    let name = name_of(grandparent, source);
                    if let Node::HtmlElement(e) = node {
                        let p = e.block_start_position;
                        cx.report(LiquidProblem::new(
                            format!("Opening tag does not have a matching closing tag for condition `{identifier}` in {in_} '{name}'"),
                            p.start,
                            p.end,
                        ));
                    } else {
                        let p = node.position();
                        cx.report(LiquidProblem::new(
                            format!("Closing tag does not have a matching opening tag for condition `{identifier}` in {in_} '{name}'"),
                            p.start,
                            p.end,
                        ));
                    }
                }
            }
        }
    }
}

/// `balanceOpenCloseTags`. A close that doesn't match stays on the stack, so nothing
/// after it matches.
fn balance<'a>(sorted: Vec<&'a Node<'a>>, source: &str) -> Vec<&'a Node<'a>> {
    let mut stack: Vec<&'a Node<'a>> = Vec::new();
    for node in sorted {
        let matches_top = matches!(node, Node::HtmlDanglingMarkerClose(_))
            && stack.last().is_some_and(|top| {
                matches!(top, Node::HtmlElement(_)) && name_of(node, source) == name_of(top, source)
            });
        if matches_top {
            stack.pop();
        } else {
            stack.push(node);
        }
    }
    stack
}

/// `getName`.
fn name_of<'n>(node: &'n Node<'n>, source: &'n str) -> &'n str {
    let parts = match node {
        Node::HtmlElement(e) => &e.name,
        Node::HtmlDanglingMarkerClose(e) => &e.name,
        Node::LiquidTag(t) => return t.name,
        _ => return node.node_type(),
    };
    match (parts.first(), parts.last()) {
        (Some(first), Some(last)) => {
            lq_syntax::js::slice(source, first.position().start, last.position().end)
        }
        _ => "",
    }
}

/// `getConditionIdentifier`. `Err` where upstream's handler throws.
fn condition_identifier(fixed: bool, branch: &lq_syntax::LiquidBranch<'_>, parent: &lq_syntax::LiquidTag<'_>) -> Result<String, JsError> {
    Ok(match (branch.name, parent.name) {
        (None, "if") => markup_identifier(fixed, &parent.markup)?,
        (None, "unless") => negate(markup_identifier(fixed, &parent.markup)?),
        (Some("else"), "if") => negate(markup_identifier(fixed, &parent.markup)?),
        (Some("else"), "unless") => markup_identifier(fixed, &parent.markup)?,
        (Some("else"), "case") => format!("case {}", markup_identifier(fixed, &parent.markup)?),
        (Some("elsif"), _) => markup_identifier(fixed, &branch.markup)?,
        (Some("when"), "case") => {
            let value = markup_identifier(fixed, &parent.markup)?;
            let conditions = when_identifier(fixed, &branch.markup)?;
            format!("case {value} == {conditions}")
        }
        _ => "??".to_string(),
    })
}

/// `getConditionIdentifierForWhenMarkup`.
fn when_identifier(fixed: bool, markup: &Markup<'_>) -> Result<String, JsError> {
    match markup {
        Markup::List(conditions) => {
            let parts = conditions.iter().map(|c| expression_identifier(fixed, c)).collect::<Result<Vec<_>, _>>()?;
            Ok(parts.join(" or "))
        }
        other => markup_identifier(fixed, other),
    }
}

fn markup_identifier(fixed: bool, markup: &Markup<'_>) -> Result<String, JsError> {
    match markup {
        Markup::Str(s) => Ok((*s).to_string()),
        Markup::Node(n) => expression_identifier(fixed, n),
        // Neither is a condition; upstream throws on `.type` and in `assertNever`.
        Markup::List(_) | Markup::None => Err(never_reached()),
    }
}

/// `getConditionIdentifierForMarkup`.
fn expression_identifier(fixed: bool, node: &Node<'_>) -> Result<String, JsError> {
    Ok(match node {
        Node::String(s) => format!("'{}'", s.value),
        Node::LiquidLiteral(l) => match l.value {
            LiteralValue::Null => "null".to_string(),
            LiteralValue::True => "true".to_string(),
            LiteralValue::False => "false".to_string(),
            LiteralValue::EmptyString => String::new(),
            // `value.toString()` of what `in` found on `Object.prototype`.
            LiteralValue::ObjectPrototype => "[object Object]".to_string(),
            LiteralValue::ObjectPrototypeFunction => {
                let name = if l.keyword == "constructor" { "Object" } else { l.keyword };
                format!("function {name}() {{ [native code] }}")
            }
        },
        Node::Number(n) => n.value.to_string(),
        Node::VariableLookup(v) => {
            // An array in a template literal: its items are joined with commas.
            let mut out = v.name.unwrap_or("").to_string();
            for (i, lookup) in v.lookups.iter().enumerate() {
                if i > 0 && !fixed {
                    out.push(',');
                }
                out.push('[');
                out.push_str(&expression_identifier(fixed, lookup)?);
                out.push(']');
            }
            out
        }
        Node::Range(r) => {
            format!("({}..{})", expression_identifier(fixed, &r.start)?, expression_identifier(fixed, &r.end)?)
        }
        Node::Comparison(c) => {
            format!("{} {} {}", expression_identifier(fixed, &c.left)?, c.comparator, expression_identifier(fixed, &c.right)?)
        }
        Node::LogicalExpression(c) => {
            format!("{} {} {}", expression_identifier(fixed, &c.left)?, c.relation.as_str(), expression_identifier(fixed, &c.right)?)
        }
        _ => return Err(never_reached()),
    })
}

/// `assertNever(x)`; only `config.onError` sees the message.
fn never_reached() -> JsError {
    JsError {
        name: "Error".to_string(),
        message: "We should never reach this code, you are missing a case for [object Object]".to_string(),
    }
}

/// `negateIdentifier`.
fn negate(identifier: String) -> String {
    match identifier.strip_prefix('-') {
        Some(rest) => rest.to_string(),
        None => format!("-{identifier}"),
    }
}
