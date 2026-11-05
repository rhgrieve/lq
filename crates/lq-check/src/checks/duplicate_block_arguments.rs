//! `DuplicateBlockArguments`.

use super::shared_args::{block_name, has_children, is_content_parameter, named, valid_block_markup};
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
        let body_overrides_content = has_children(tag);
        let mut seen: Vec<&str> = Vec::new();
        for arg in &markup.args {
            let Some(a) = named(arg) else { continue };
            let p = arg.position();
            if seen.contains(&&*a.name) {
                let message =
                    format!("Duplicate argument '{}' in block tag for '{}'.", a.name, block_name(markup));
                cx.report(LiquidProblem::new(message, p.start, p.end));
            }
            seen.push(&a.name);
            if body_overrides_content && is_content_parameter(&a.name) {
                cx.report(LiquidProblem::new(
                    "The explicit 'content' argument has no effect because the inline block body takes precedence.",
                    p.start,
                    p.end,
                ));
            }
        }
    }
}
