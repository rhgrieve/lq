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

/// `valueIncludes(attr, '.+cdn.shopify.com.+')`: the word is a regular expression.
fn shopify_cdn_regex() -> &'static JsRegex {
    static RE: OnceLock<JsRegex> = OnceLock::new();
    RE.get_or_init(|| JsRegex::new(r"(^|\s).+cdn.shopify.com.+(\s|$)", "").expect("valid regex"))
}

/// The lowercased hostname of an absolute or protocol-relative http(s) URL.
fn url_hostname(url: &str) -> Option<String> {
    let url = lq_syntax::js::trim(url);
    let lower = url.get(..8).map(str::to_ascii_lowercase).unwrap_or_default();
    let rest = if lower.starts_with("https://") {
        &url[8..]
    } else if lower.starts_with("http://") {
        &url[7..]
    } else {
        url.strip_prefix("//")?
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    Some(host.split(':').next().unwrap_or("").to_ascii_lowercase())
}

/// [`Divergence::CdnPreconnectHost`].
fn is_shopify_cdn_url(url: &str) -> bool {
    url_hostname(url).as_deref() == Some("cdn.shopify.com")
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
        let fixed = cx.divergence(Divergence::CdnPreconnectHost);
        let is_shopify_cdn = attributes.iter().any(|a| {
            is_attr(a, "href")
                && valued_attribute(a).is_some_and(|v| {
                    if fixed {
                        // The URL's start is what decides its host, so only a leading text node.
                        return matches!(v.value.first(), Some(Node::TextNode(t)) if is_shopify_cdn_url(&t.value));
                    }
                    v.value.iter().any(|n| matches!(n, Node::TextNode(t) if shopify_cdn_regex().is_match(&t.value)))
                })
        });
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
