//! `HardcodedRoutes`.

use super::shared_rules::type_error;
use crate::api::*;
use crate::check_utils::{is_attr, valued_attribute};
use lq_syntax::{AttrNode, Node};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::HtmlElement, NodeKind::LiquidFilter]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(HardcodedRoutes))
}

struct HardcodedRoutes;

/// `HARDCODED_ROUTES`: the keys of `ROUTES` reversed, with their route names.
const HARDCODED_ROUTES: [(&str, &str); 18] = [
    ("/search/suggest", "predictive_search_url"),
    ("/search", "search_url"),
    ("/recommendations/products", "product_recommendations_url"),
    ("/customer_authentication/login", "storefront_login_url"),
    ("/collections/all", "all_products_collection_url"),
    ("/collections", "collections_url"),
    ("/cart/update", "cart_update_url"),
    ("/cart/clear", "cart_clear_url"),
    ("/cart/change", "cart_change_url"),
    ("/cart/add", "cart_add_url"),
    ("/cart", "cart_url"),
    ("/account/register", "account_register_url"),
    ("/account/recover", "account_recover_url"),
    ("/account/logout", "account_logout_url"),
    ("/account/login", "account_login_url"),
    ("/account/addresses", "account_addresses_url"),
    ("/account", "account_url"),
    ("/", "root_url"),
];

/// `value.startsWith(route)`. With `bounded` ([`Divergence::RouteBoundary`]) the route must
/// end at a segment boundary, so `/cartoons` isn't `/cart`.
fn route_prefix(value: &str, route: &str, bounded: bool) -> bool {
    match value.strip_prefix(route) {
        None => false,
        Some(rest) => !bounded || rest.is_empty() || matches!(rest.as_bytes()[0], b'/' | b'?' | b'#'),
    }
}

fn hardcoded_route(attr: &AttrNode<'_>, bounded: bool) -> Option<(&'static str, &'static str)> {
    let Some(Node::TextNode(text)) = attr.value.first() else { return None };
    HARDCODED_ROUTES.iter().copied().find(|(route, _)| {
        if *route == "/" {
            text.value == "/" && attr.value.len() == 1
        } else {
            route_prefix(&text.value, route, bounded)
        }
    })
}

fn report(cx: &LiquidCx<'_, '_>, (route, url): (&str, &str), start: u32) {
    cx.report(LiquidProblem::new(
        format!("Use routes object {{{{ routes.{url} }}}} instead of hardcoding {route}"),
        start,
        start + route.len() as u32,
    ));
}

impl<'a> LiquidCheck<'a> for HardcodedRoutes {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        match node {
            Node::HtmlElement(element) => {
                let bounded = cx.divergence(Divergence::RouteBoundary);
                let found = element.attributes.iter().find_map(|a| {
                    let valued = valued_attribute(a)?;
                    if !is_attr(a, "action") && !is_attr(a, "href") {
                        return None;
                    }
                    Some((hardcoded_route(valued, bounded)?, valued.attribute_position.start))
                });
                if let Some((route, start)) = found {
                    report(cx, route, start);
                }
            }
            Node::LiquidFilter(filter) if filter.name == "link_to" => {
                let Some(arg) = filter.args.first() else {
                    if cx.divergence(Divergence::LinkToNoArgument) {
                        return;
                    }
                    cx.fail(type_error("Cannot read properties of undefined (reading 'type')"));
                    return;
                };
                let value = match arg {
                    Node::String(s) => s.value,
                    _ => "",
                };
                let bounded = cx.divergence(Divergence::RouteBoundary);
                let route = HARDCODED_ROUTES.iter().copied().find(|(route, _)| {
                    if *route == "/" {
                        value == "/"
                    } else {
                        route_prefix(value, route, bounded)
                    }
                });
                if let Some(route) = route {
                    // One past the opening quote.
                    report(cx, route, arg.position().start + 1);
                }
            }
            _ => {}
        }
    }
}
