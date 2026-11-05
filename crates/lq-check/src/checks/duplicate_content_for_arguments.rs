//! `DuplicateContentForArguments`.

use super::shared_args::named;
use crate::api::*;
use crate::liquid_doc::arguments::{block_name, report_duplicate_arguments};
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
        let mut seen: Vec<&str> = Vec::new();
        let mut duplicates = Vec::new();
        for arg in &markup.args {
            let Some(a) = named(arg) else { continue };
            if seen.contains(&&*a.name) {
                duplicates.push(arg);
            }
            seen.push(&a.name);
        }
        report_duplicate_arguments(cx, node, &duplicates, name);
    }
}
