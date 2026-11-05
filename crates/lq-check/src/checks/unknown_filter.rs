//! `UnknownFilter`.

use crate::api::*;
use crate::check_utils::{closest, did_you_mean};
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidFilter]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    cx.docset()?;
    Some(Box::new(UnknownFilter))
}

struct UnknownFilter;

impl<'a> LiquidCheck<'a> for UnknownFilter {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidFilter(filter) = node else { return };
        let Some(docset) = cx.docset() else { return };
        if docset.filter(filter.name).is_none() {
            let p = node.position();
            cx.report(
                LiquidProblem::new(format!("Unknown filter '{}' used.", filter.name), p.start + 1, p.end).hints_if(cx.wants_hints(), |problem| {
                    problem.note_opt(closest(filter.name, docset.filters().iter().map(|f| f.name)).map(did_you_mean))
                }),
            );
        }
    }
}
