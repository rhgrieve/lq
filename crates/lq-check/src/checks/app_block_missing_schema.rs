//! `AppBlockMissingSchema`.

use crate::api::*;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    if !cx.to_relative_path(cx.uri()).starts_with("blocks/") {
        return None;
    }
    Some(Box::new(AppBlockMissingSchema { found_schema: false }))
}

struct AppBlockMissingSchema {
    found_schema: bool,
}

impl<'a> LiquidCheck<'a> for AppBlockMissingSchema {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], _cx: &LiquidCx<'a, '_>) {
        if matches!(node, Node::LiquidRawTag(t) if t.name == "schema") {
            self.found_schema = true;
        }
    }

    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {
        if !self.found_schema {
            cx.report(LiquidProblem::new("The schema does not exist", 0, 0).file_level());
        }
    }
}
