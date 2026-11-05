//! `UniqueDocParamNames`.

use crate::api::*;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidDocParamNode]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::<Check<'a>>::default())
}

#[derive(Default)]
struct Check<'a> {
    defined: Vec<&'a str>,
}

impl<'a> LiquidCheck<'a> for Check<'a> {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidDocParamNode(param) = node else { return };
        let Node::TextNode(name) = &param.param_name else { return };
        let name: &'a str = &name.value;
        if !self.defined.contains(&name) {
            self.defined.push(name);
            return;
        }
        let p = param.param_name.position();
        cx.report(LiquidProblem::new(format!("The parameter '{name}' is defined more than once."), p.start, p.end));
    }
}
