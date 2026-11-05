//! `MissingAsset`.

use super::shared_rules::filter_name;
use crate::api::*;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidVariable]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(MissingAsset))
}

struct MissingAsset;

impl<'a> LiquidCheck<'a> for MissingAsset {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidVariable(variable) = node else { return };
        if variable.filters.first().map(filter_name) != Some("asset_url") {
            return;
        }
        let Node::String(expression) = &variable.expression else { return };
        let original = format!("assets/{}", expression.value);
        if cx.does_file_exist(&original) {
            return;
        }
        let mut path = original.clone();
        if path.ends_with(".scss.css") {
            path = path.replacen(".scss.css", ".scss.liquid", 1);
            if cx.does_file_exist(&path) {
                return;
            }
        }
        if path.ends_with(".js") || path.ends_with(".css") {
            path.push_str(".liquid");
            if cx.does_file_exist(&path) {
                return;
            }
        }
        let p = variable.expression.position();
        cx.report(LiquidProblem::new(format!("'{original}' does not exist"), p.start, p.end));
    }
}
