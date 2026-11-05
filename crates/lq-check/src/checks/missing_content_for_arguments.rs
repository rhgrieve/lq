//! `MissingContentForArguments`.

use super::shared_args::named;
use crate::api::*;
use crate::liquid_doc::arguments::{block_name, liquid_doc_params, report_missing_arguments};
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
        let missing: Vec<_> = params
            .values()
            .filter(|p| p.required && !markup.args.iter().filter_map(named).any(|a| a.name == p.name))
            .collect();
        report_missing_arguments(cx, node, &missing, name);
    }
}
