//! `ValidStaticBlockType`.

use crate::api::*;
use crate::check_utils::is_content_for_block;
use lq_syntax::{Markup, Node};

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
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidTag(tag) = node else { return };
        if tag.name != "content_for" || !is_content_for_block(&tag.markup) {
            return;
        }
        let Markup::Node(markup) = &tag.markup else { return };
        let Node::ContentForMarkup(markup) = &**markup else { return };
        let type_arg = markup.args.iter().find_map(|a| match a {
            Node::NamedArgument(n) if n.name == "type" => Some(n),
            _ => None,
        });
        let Some(type_arg) = type_arg else { return };
        let Node::String(block_name) = &type_arg.value else { return };
        if !cx.does_file_exist(&format!("blocks/{}.liquid", block_name.value)) {
            cx.report(
                LiquidProblem::new(
                    format!("'blocks/{}.liquid' does not exist", block_name.value),
                    block_name.position.start,
                    block_name.position.end,
                )
                .with_empty_suggestions(),
            );
        }
    }
}
