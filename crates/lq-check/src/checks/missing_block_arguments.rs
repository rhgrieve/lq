//! `MissingBlockArguments`.

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
        let name = block_name(markup);
        let Some(parameters) = cx.block_parameters(name) else { return };
        let body_is_content = has_children(tag);
        for parameter in &parameters.0 {
            if !parameter.required
                || markup.args.iter().filter_map(named).any(|a| a.name == parameter.name)
                || (body_is_content && is_content_parameter(&parameter.name))
            {
                continue;
            }
            let p = tag.block_start_position;
            cx.report(LiquidProblem::new(
                format!("Missing required argument '{}' in block tag for '{name}'.", parameter.name),
                p.start,
                p.end,
            ).hints_if(cx.wants_hints(), |p| p.note(format!("blocks/{name}.liquid declares it as required"))));
        }
    }
}
