//! `DeprecateBgsizes`.

use super::shared_rules::find_valued;
use crate::api::*;
use crate::check_utils::{is_attr, valued_attribute, value_includes};
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::HtmlElement]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(DeprecateBgsizes))
}

struct DeprecateBgsizes;

impl<'a> LiquidCheck<'a> for DeprecateBgsizes {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::HtmlElement(element) = node else { return };
        let lazyload = element
            .attributes
            .iter()
            .find_map(|a| valued_attribute(a).filter(|v| is_attr(a, "class") && value_includes(v, "lazyload")));
        if let Some(attr) = lazyload {
            let p = attr.attribute_position;
            cx.report(LiquidProblem::new(
                "Use the native loading=\"lazy\" attribute instead of lazysizes",
                p.start,
                p.end,
            ));
        }
        if let Some(attr) = find_valued(&element.attributes, "data-bgset") {
            let p = attr.position;
            cx.report(LiquidProblem::new("Use the CSS imageset attribute instead of data-bgset", p.start, p.end));
        }
    }
}
