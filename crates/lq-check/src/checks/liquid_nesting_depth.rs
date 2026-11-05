//! `LiquidNestingDepth`.

use crate::api::*;
use lq_json::js::number_to_string;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(LiquidNestingDepth { max: cx.settings().f64("maxDepth").unwrap_or(f64::NAN) }))
}

struct LiquidNestingDepth {
    max: f64,
}

fn is_nesting_tag(name: &str) -> bool {
    matches!(name, "if" | "unless" | "for" | "case" | "tablerow")
}

impl<'a> LiquidCheck<'a> for LiquidNestingDepth {
    fn enter(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidTag(tag) = node else { return };
        if !is_nesting_tag(tag.name) {
            return;
        }
        let depth = 1 + ancestors.iter().filter(|a| matches!(a, Node::LiquidTag(t) if is_nesting_tag(t.name))).count();
        if depth as f64 <= self.max {
            return;
        }
        let p = tag.block_start_position;
        cx.report(LiquidProblem::new(
            format!(
                "This Liquid block is nested {depth} levels deep, which exceeds the maximum allowed depth of {}.",
                number_to_string(self.max)
            ),
            p.start,
            p.end,
        ));
    }
}
