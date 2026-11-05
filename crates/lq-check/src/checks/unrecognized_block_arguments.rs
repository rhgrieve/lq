//! `UnrecognizedBlockArguments`.

use super::shared_args::{block_name, named, valid_block_markup};
use crate::api::*;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check))
}

struct Check;

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidTag(tag) = node else { return };
        let Some(markup) = valid_block_markup(tag, cx.source()) else { return };
        let name = block_name(markup);
        let Some(parameters) = cx.block_parameters(name) else { return };
        for arg in &markup.args {
            let Some(a) = named(arg) else { continue };
            if a.name == "block.name" || parameters.get(&a.name).is_some() {
                continue;
            }
            let p = arg.position();
            cx.report(LiquidProblem::new(
                format!("Unknown argument '{}' in block tag for '{name}'.", a.name),
                p.start,
                p.end,
            ));
        }
    }
}
