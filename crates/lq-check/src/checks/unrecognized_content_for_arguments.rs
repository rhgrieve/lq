//! `UnrecognizedContentForArguments`.

use super::shared_args::named;
use crate::api::*;
use crate::check_utils::{CLOSEST_ARGUMENT, REQUIRED_CONTENT_FOR_ARGUMENTS, RESERVED_CONTENT_FOR_ARGUMENTS};
use crate::liquid_doc::arguments::{block_name, liquid_doc_params, report_unknown_arguments};
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::ContentForMarkup]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check))
}

struct Check;

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::ContentForMarkup(markup) = node else { return };
        let Some(name) = block_name(node) else { return };
        let Some(params) = liquid_doc_params(cx, &format!("blocks/{name}.liquid")) else { return };
        let unknown: Vec<_> = markup
            .args
            .iter()
            .filter(|arg| {
                named(arg).is_some_and(|a| {
                    !params.has(&a.name)
                        && !RESERVED_CONTENT_FOR_ARGUMENTS.contains(&&*a.name)
                        && !REQUIRED_CONTENT_FOR_ARGUMENTS.contains(&&*a.name)
                        && !a.name.starts_with(CLOSEST_ARGUMENT)
                })
            })
            .collect();
        report_unknown_arguments(cx, node, &unknown, name, &params);
    }
}
