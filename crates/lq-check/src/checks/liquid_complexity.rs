//! `LiquidComplexity`.

use crate::api::*;
use lq_json::js::number_to_string;
use lq_syntax::{Node, Position};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag, NodeKind::LiquidBranch, NodeKind::LogicalExpression]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(LiquidComplexity {
        max: cx.settings().f64("maxComplexity").unwrap_or(f64::NAN),
        complexity: 1,
        first_over: None,
    }))
}

struct LiquidComplexity {
    max: f64,
    complexity: u32,
    first_over: Option<Position>,
}

impl LiquidComplexity {
    fn increment(&mut self, range: Position) {
        self.complexity += 1;
        if self.first_over.is_none() && f64::from(self.complexity) > self.max {
            self.first_over = Some(range);
        }
    }
}

fn in_control_flow_condition(ancestors: &[&Node<'_>]) -> bool {
    for ancestor in ancestors.iter().rev() {
        match ancestor {
            Node::LiquidTag(tag) => return matches!(tag.name, "if" | "unless"),
            Node::LiquidBranch(branch) => return branch.name == Some("elsif"),
            Node::LiquidVariableOutput(_) => return false,
            _ => {}
        }
    }
    false
}

impl<'a> LiquidCheck<'a> for LiquidComplexity {
    fn enter(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], _cx: &LiquidCx<'a, '_>) {
        match node {
            Node::LiquidTag(tag) => {
                if matches!(tag.name, "if" | "unless" | "case" | "for" | "tablerow" | "paginate") {
                    self.increment(tag.block_start_position);
                }
            }
            Node::LiquidBranch(branch) => {
                if matches!(branch.name, Some("elsif" | "when")) {
                    self.increment(branch.block_start_position);
                }
            }
            Node::LogicalExpression(_) if in_control_flow_condition(ancestors) => {
                self.increment(node.position());
            }
            _ => {}
        }
    }

    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {
        if f64::from(self.complexity) <= self.max {
            return;
        }
        let source = cx.source();
        let range = self.first_over.unwrap_or(Position::new(0, source.len() as u32));
        let line = 1 + source.as_bytes()[..range.start as usize].iter().filter(|&&b| b == b'\n').count();
        cx.report(LiquidProblem::new(
            format!(
                "Liquid complexity is {}, which exceeds the maximum of {}. The decision at line {line} pushed it over the limit. Consider simplifying conditional logic, or moving self-contained decision logic into snippets with the render tag.",
                self.complexity,
                number_to_string(self.max),
            ),
            range.start,
            range.end,
        ));
    }
}
