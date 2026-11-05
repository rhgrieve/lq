//! `DeprecatedTag`.

use crate::api::*;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(DeprecatedTag))
}

struct DeprecatedTag;

impl<'a> LiquidCheck<'a> for DeprecatedTag {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidTag(tag) = node else { return };
        if tag.name != "include" {
            return;
        }
        let tag_start = node.position().start;
        let Some(offset) = cx.source().get(tag_start as usize..).and_then(|s| s.find("include")) else { return };
        let start = tag_start + offset as u32;
        let end = start + "include".len() as u32;
        cx.report(
            LiquidProblem::new("Use the 'render' tag instead of 'include'", start, end)
                .suggest("Replace 'include' with 'render'", |c| c.replace(start, end, "render")),
        );
    }
}
