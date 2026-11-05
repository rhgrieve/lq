//! `DeprecateLazysizes`.

use crate::api::*;
use crate::check_utils::{html_attributes, html_node_name, is_attr, valued_attribute, value_includes};
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::HtmlVoidElement, NodeKind::HtmlSelfClosingElement]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(DeprecateLazysizes))
}

struct DeprecateLazysizes;

impl<'a> LiquidCheck<'a> for DeprecateLazysizes {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        if html_node_name(node) != Some("img") {
            return;
        }
        let attributes = html_attributes(node);
        if attributes.iter().any(|a| is_attr(a, "src")) && attributes.iter().any(|a| is_attr(a, "loading")) {
            return;
        }
        let has_lazyload_class = attributes
            .iter()
            .any(|a| is_attr(a, "class") && valued_attribute(a).is_some_and(|v| value_includes(v, "lazyload")));
        if !has_lazyload_class {
            return;
        }
        let has_lazysizes_attribute = attributes
            .iter()
            .any(|a| valued_attribute(a).is_some() && (is_attr(a, "data-srcset") || is_attr(a, "data-sizes")));
        if !has_lazysizes_attribute {
            return;
        }
        let p = node.position();
        cx.report(LiquidProblem::new(
            "Use the native loading=\"lazy\" attribute instead of lazysizes",
            p.start,
            p.end,
        ));
    }
}
