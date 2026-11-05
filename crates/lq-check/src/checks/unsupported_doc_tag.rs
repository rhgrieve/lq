//! `UnsupportedDocTag`.

use crate::api::*;
use crate::liquid_doc::file_path_supports_liquid_doc;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    if file_path_supports_liquid_doc(cx.uri()) {
        return None;
    }
    Some(Box::new(Check))
}

struct Check;

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidRawTag(tag) = node else { return };
        if tag.name != "doc" {
            return;
        }
        let p = node.position();
        cx.report(
            LiquidProblem::new("The `doc` tag can only be used within a snippet or block.", p.start, p.end)
                .suggest("Remove unsupported `doc` tag", |c| c.remove(p.start, p.end)),
        );
    }
}
