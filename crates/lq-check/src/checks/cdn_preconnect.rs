//! `CdnPreconnect`.

use crate::api::*;
use crate::check_utils::{html_attributes, html_node_name, is_attr, valued_attribute, value_includes};
use lq_json::regex::JsRegex;
use lq_syntax::Node;
use std::sync::OnceLock;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::HtmlVoidElement, NodeKind::HtmlSelfClosingElement]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(CdnPreconnect))
}

struct CdnPreconnect;

/// `SHOPIFY_CDN_ORIGIN`, `/^(https?:)?\/\/cdn\.shopify\.com(:443)?/i`.
fn shopify_cdn_origin() -> &'static JsRegex {
    static RE: OnceLock<JsRegex> = OnceLock::new();
    RE.get_or_init(|| JsRegex::new(r"^(https?:)?\/\/cdn\.shopify\.com(:443)?", "i").expect("valid regex"))
}

/// `isShopifyCdnUrl(attr)`: the href starts with the cdn.shopify.com origin in static text, and
/// the host ends there: at a path, query or fragment, or at the end of the whole value.
fn is_shopify_cdn_url(value: &[Node<'_>]) -> bool {
    let Some(Node::TextNode(first)) = value.first() else { return false };
    let url = lq_syntax::js::trim(&first.value);
    let Some(origin) = shopify_cdn_origin().exec_at(url, 0) else { return false };
    match &url[origin.end()..] {
        "" => value.len() == 1,
        after => after.starts_with(['/', '?', '#']),
    }
}

impl<'a> LiquidCheck<'a> for CdnPreconnect {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        if html_node_name(node) != Some("link") {
            return;
        }
        let attributes = html_attributes(node);
        let is_preconnect = attributes
            .iter()
            .any(|a| is_attr(a, "rel") && valued_attribute(a).is_some_and(|v| value_includes(v, "preconnect")));
        if !is_preconnect {
            return;
        }
        let is_shopify_cdn = attributes
            .iter()
            .any(|a| is_attr(a, "href") && valued_attribute(a).is_some_and(|v| is_shopify_cdn_url(&v.value)));
        if !is_shopify_cdn {
            return;
        }
        let p = node.position();
        cx.report(LiquidProblem::new(
            "Preconnecting to cdn.shopify.com is unnecessary and can lead to worse performance",
            p.start,
            p.end,
        ));
    }
}
