//! `AssetSizeCSS`. The remote path never reports: `lq` has no network, see
//! `has_remote_asset_size_exceeded_threshold`.

use super::shared_rules::{check_asset_size_attribute, check_asset_size_filter, find_valued};
use crate::api::*;
use crate::check_utils::{is_attr, valued_attribute, value_includes};
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::HtmlVoidElement, NodeKind::LiquidFilter]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(AssetSizeCss))
}

struct AssetSizeCss;

fn message(threshold: &str) -> String {
    format!("The CSS file size exceeds the threshold of {threshold} bytes")
}

impl<'a> LiquidCheck<'a> for AssetSizeCss {
    fn enter(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        match node {
            Node::HtmlVoidElement(element) if element.name == "link" => {
                let is_stylesheet = element
                    .attributes
                    .iter()
                    .any(|a| is_attr(a, "rel") && valued_attribute(a).is_some_and(|v| value_includes(v, "stylesheet")));
                if !is_stylesheet {
                    return;
                }
                if let Some(href) = find_valued(&element.attributes, "href") {
                    check_asset_size_attribute(cx, href, message);
                }
            }
            Node::LiquidFilter(filter) if filter.name == "stylesheet_tag" => {
                check_asset_size_filter(cx, "stylesheet_tag", ancestors, message);
            }
            _ => {}
        }
    }
}
