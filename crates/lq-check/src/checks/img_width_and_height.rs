//! `ImgWidthAndHeight`.

use super::shared_rules::find_valued;
use crate::api::*;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::HtmlVoidElement]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(ImgWidthAndHeight))
}

struct ImgWidthAndHeight;

impl<'a> LiquidCheck<'a> for ImgWidthAndHeight {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::HtmlVoidElement(element) = node else { return };
        if element.name != "img" {
            return;
        }
        let width = find_valued(&element.attributes, "width").is_some();
        let height = find_valued(&element.attributes, "height").is_some();
        let message = match (width, height) {
            (true, true) => return,
            (false, true) => "Missing width attribute on img tag",
            (true, false) => "Missing height attribute on img tag",
            (false, false) => "Missing width and height attributes on img tag",
        };
        let p = node.position();
        cx.report(LiquidProblem::new(message, p.start, p.end));
    }
}
