//! `AssetSizeJavaScript`. The remote path never reports: `lq` has no network, see
//! `has_remote_asset_size_exceeded_threshold`.

use super::shared_rules::{check_asset_size_attribute, check_asset_size_filter, find_valued};
use crate::api::*;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::HtmlRawNode, NodeKind::LiquidFilter]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(AssetSizeJavaScript))
}

struct AssetSizeJavaScript;

fn message(threshold: &str) -> String {
    format!("JavaScript on every page load exceeds compressed size threshold ({threshold} Bytes), consider using the import on interaction pattern.")
}

impl<'a> LiquidCheck<'a> for AssetSizeJavaScript {
    fn enter(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        match node {
            Node::HtmlRawNode(element) if element.name == "script" => {
                if let Some(src) = find_valued(&element.attributes, "src") {
                    check_asset_size_attribute(cx, src, message);
                }
            }
            Node::LiquidFilter(filter) if filter.name == "script_tag" => {
                check_asset_size_filter(cx, "script_tag", ancestors, message);
            }
            _ => {}
        }
    }
}
