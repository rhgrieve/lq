//! `ValidStandardEventData`.

use super::shared_schema::OBJECT_PROTOTYPE_KEYS;
use crate::api::*;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidFilter]),
    exit: NodeMask::NONE,
};

const SUPPORTED_EVENT_TYPE: &str = "view";
const PRODUCT_CONTEXTS: [&str; 5] = ["page", "search", "collection", "dialog", "recommendation"];
const CART_CONTEXTS: [&str; 2] = ["page", "dialog"];
/// `CONTEXTS_SUPPORTED_BY_ANY_INPUT_TYPE`.
const ANY_CONTEXTS: [&str; 5] = PRODUCT_CONTEXTS;

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check))
}

struct Check;

fn is_invalid_static_value(value: &Node<'_>, supported: &[&str]) -> bool {
    match value {
        Node::VariableLookup(_) => false,
        Node::String(s) => !supported.contains(&s.value),
        _ => true,
    }
}

fn describe_value(value: &Node<'_>) -> String {
    match value {
        Node::String(s) => format!(" '{}'", s.value),
        _ => String::new(),
    }
}

fn detect_input_drop<'n>(node: &Node<'_>, parent: Option<&&'n Node<'n>>) -> Option<&'n str> {
    let Node::LiquidVariable(parent) = *parent? else { return None };
    if !parent.filters.first().is_some_and(|f| std::ptr::addr_eq(f, node)) {
        return None;
    }
    let Node::VariableLookup(lookup) = &parent.expression else { return None };
    if !lookup.lookups.is_empty() {
        return None;
    }
    lookup.name
}

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidFilter(filter) = node else { return };
        if filter.name != "standard_event_data" {
            return;
        }
        if let Some(event_type) = filter.args.iter().find(|a| !matches!(a, Node::NamedArgument(_))) {
            if is_invalid_static_value(event_type, &[SUPPORTED_EVENT_TYPE]) {
                let p = event_type.position();
                cx.report(LiquidProblem::new(
                    format!(
                        "Unsupported event type{}. The only supported event type is '{SUPPORTED_EVENT_TYPE}'.",
                        describe_value(event_type)
                    ),
                    p.start,
                    p.end,
                ));
            }
        }

        let drop = detect_input_drop(node, ancestors.last());
        if drop == Some("collection") {
            return;
        }
        let context_value = filter.args.iter().find_map(|a| match a {
            Node::NamedArgument(n) if n.name == "context" => Some(&n.value),
            _ => None,
        });
        let Some(context_value) = context_value else { return };

        let by_drop: Option<&[&str]> = match drop {
            Some("product") => Some(&PRODUCT_CONTEXTS),
            Some("cart") => Some(&CART_CONTEXTS),
            _ => None,
        };
        // `SUPPORTED_CONTEXTS_BY_DROP[drop]` finds `Object.prototype` members, and calling
        // array methods on them throws unless the value is a lookup.
        if drop.is_some_and(|d| OBJECT_PROTOTYPE_KEYS.contains(&d)) {
            if !matches!(context_value, Node::VariableLookup(_)) {
                cx.fail(super::shared_schema::type_error("supportedValues.includes is not a function"));
            }
            return;
        }
        let supported = by_drop.unwrap_or(&ANY_CONTEXTS);
        if is_invalid_static_value(context_value, supported) {
            let drop_description = match (drop, by_drop) {
                (Some(d), Some(_)) => format!(" for {d}"),
                _ => String::new(),
            };
            let p = context_value.position();
            cx.report(LiquidProblem::new(
                format!(
                    "Unsupported context{}{drop_description}. Valid values: {}. The 'context' argument can also be omitted.",
                    describe_value(context_value),
                    supported.join(", ")
                ),
                p.start,
                p.end,
            ));
        }
    }
}
