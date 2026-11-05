//! `ReservedDocParamNames`.

use crate::api::*;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidDocParamNode]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check))
}

struct Check;

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidDocParamNode(param) = node else { return };
        let Node::TextNode(name) = &param.param_name else { return };
        // `Object.keys(LiquidLiteralValues)`.
        if !matches!(&*name.value, "nil" | "null" | "true" | "false" | "blank" | "empty") {
            return;
        }
        let p = param.param_name.position();
        cx.report(LiquidProblem::new(
            format!("The parameter name '{}' is reserved because Liquid parses it as a literal.", name.value),
            p.start,
            p.end,
        ));
    }
}
