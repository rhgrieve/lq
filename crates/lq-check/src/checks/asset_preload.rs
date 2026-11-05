//! `AssetPreload`.

use crate::api::*;
use crate::check_utils::{has_attribute_value_of, is_attr, valued_attribute};
use lq_syntax::{AttrNode, Node};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::HtmlVoidElement]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(AssetPreload))
}

struct AssetPreload;

fn is_preload(attr: &Node<'_>, valued: &AttrNode<'_>) -> bool {
    is_attr(attr, "rel") && valued.value.iter().any(|n| matches!(n, Node::TextNode(t) if t.value == "preload"))
}

impl<'a> LiquidCheck<'a> for AssetPreload {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::HtmlVoidElement(element) = node else { return };
        if element.name != "link" {
            return;
        }
        let mut valued = element.attributes.iter().filter_map(|a| valued_attribute(a).map(|v| (a, v)));
        if !valued.clone().any(|(a, v)| is_preload(a, v)) {
            return;
        }
        let has = |name: &str, value: &str| {
            valued.clone().any(|(a, v)| is_attr(a, name) && has_attribute_value_of(v, value))
        };
        if has("as", "image") && has("fetchpriority", "high") {
            return;
        }
        let asset_type = valued.find(|(a, _)| is_attr(a, "as")).and_then(|(_, v)| {
            v.value.iter().find_map(|n| match n {
                Node::TextNode(t) => Some(&*t.value),
                _ => None,
            })
        });
        let message = match asset_type {
            Some("style") => "For better performance, prefer using the preload argument of the stylesheet_tag filter",
            Some("image") => "For better performance, prefer using the preload argument of the image_tag filter",
            _ => "For better performance, prefer using the preload_tag filter",
        };
        let p = node.position();
        cx.report(LiquidProblem::new(message, p.start, p.end));
    }
}
