//! `ContentForHeaderModification`.

use crate::api::*;
use lq_syntax::{Markup, Node, Position};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag, NodeKind::LiquidVariableOutput]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(ContentForHeaderModification))
}

struct ContentForHeaderModification;

fn check(cx: &LiquidCx<'_, '_>, expression: &Node<'_>, position: Position) {
    if matches!(expression, Node::VariableLookup(l) if l.name == Some("content_for_header")) {
        cx.report(LiquidProblem::new(
            "Do not rely on the content of `content_for_header`",
            position.start,
            position.end,
        ));
    }
}

fn check_variable(cx: &LiquidCx<'_, '_>, variable: &Node<'_>, position: Position) {
    if let Node::LiquidVariable(v) = variable {
        check(cx, &v.expression, position);
    }
}

impl<'a> LiquidCheck<'a> for ContentForHeaderModification {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        match node {
            Node::LiquidTag(tag) => {
                let Markup::Node(markup) = &tag.markup else { return };
                match (tag.name, &**markup) {
                    ("assign", Node::AssignMarkup(assign)) => check_variable(cx, &assign.value, node.position()),
                    ("echo", variable) => check_variable(cx, variable, node.position()),
                    ("capture", _) => {
                        for child in tag.children.as_deref().unwrap_or(&[]) {
                            if let Node::LiquidVariableOutput(output) = child {
                                if let Markup::Node(variable) = &output.markup {
                                    check_variable(cx, variable, child.position());
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Node::LiquidVariableOutput(output) => {
                if let Markup::Node(markup) = &output.markup {
                    if let Node::LiquidVariable(v) = &**markup {
                        if !v.filters.is_empty() {
                            check(cx, &v.expression, node.position());
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
