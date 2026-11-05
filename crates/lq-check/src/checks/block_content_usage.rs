//! `BlockContentUsage`.

use super::shared_args::is_content_parameter;
use crate::api::*;
use crate::check_utils::is_within_unparsed_raw_tag;
use crate::to_schema::is_block;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::VariableLookup]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    if !is_block(cx.uri()) {
        return None;
    }
    Some(Box::new(Check))
}

struct Check;

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::VariableLookup(lookup) = node else { return };
        if lookup.name != Some("block")
            || !matches!(lookup.lookups.first(), Some(Node::String(s)) if is_content_parameter(s.value))
            || is_within_unparsed_raw_tag(ancestors)
        {
            return;
        }
        let p = node.position();
        cx.report(LiquidProblem::new("Use the implicit 'content' parameter directly instead of 'block.content'.", p.start, p.end));
    }
}
