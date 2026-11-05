//! `ValidContentForArguments`.

use super::shared_args::named;
use crate::api::*;
use crate::check_utils::{CLOSEST_ARGUMENT, REQUIRED_CONTENT_FOR_ARGUMENTS, RESERVED_CONTENT_FOR_ARGUMENTS};
use lq_json::validator::JsError;
use lq_syntax::{ContentForMarkup, Markup, Node};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check))
}

struct Check;

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidTag(tag) = node else { return };
        if tag.name != "content_for" {
            return;
        }
        let Markup::Node(markup_node) = &tag.markup else { return };
        let Node::ContentForMarkup(markup) = &**markup_node else { return };
        let Node::String(ty) = &markup.content_for_type else { return };
        match ty.value {
            "blocks" => validate_blocks(markup, cx),
            "block" => validate_block(markup, markup_node, cx),
            // `validationStrategies[type]` is a plain object lookup, so it also finds
            // `Object.prototype`. These throw when called without a receiver; `toString`
            // and `constructor` don't.
            "__proto__" | "__defineGetter__" | "__defineSetter__" | "__lookupGetter__" | "__lookupSetter__"
            | "hasOwnProperty" | "isPrototypeOf" | "propertyIsEnumerable" | "toLocaleString" | "valueOf"
                if !cx.divergence(Divergence::ContentForPrototype) =>
            {
                cx.fail(JsError { name: "TypeError".into(), message: format!("validate ({}) threw", ty.value) });
            }
            _ => {}
        }
    }
}

fn validate_blocks<'a>(markup: &ContentForMarkup<'a>, cx: &LiquidCx<'a, '_>) {
    for arg in &markup.args {
        if named(arg).is_some_and(|a| !a.name.starts_with(CLOSEST_ARGUMENT)) {
            let p = arg.position();
            cx.report(LiquidProblem::new("{% content_for \"blocks\" %} only accepts 'closest.*' arguments", p.start, p.end));
        }
    }
}

fn validate_block<'a>(markup: &ContentForMarkup<'a>, node: &Node<'a>, cx: &LiquidCx<'a, '_>) {
    for required in REQUIRED_CONTENT_FOR_ARGUMENTS {
        let Some(arg) = markup.args.iter().filter_map(named).find(|a| a.name == required) else {
            let p = node.position();
            cx.report(
                LiquidProblem::new(format!("{{% content_for \"block\" %}} requires a '{required}' argument"), p.start, p.end)
                    .with_empty_suggestions(),
            );
            continue;
        };
        if !matches!(arg.value, Node::String(_)) {
            let p = arg.value.position();
            cx.report(
                LiquidProblem::new(format!("The '{required}' argument should be a string"), p.start, p.end)
                    .with_empty_suggestions(),
            );
        }
    }
    for arg in &markup.args {
        let Some(a) = named(arg) else { continue };
        if RESERVED_CONTENT_FOR_ARGUMENTS.contains(&&*a.name) {
            let p = arg.position();
            cx.report(LiquidProblem::new(
                format!("{{% content_for \"block\" %}} doesn't support '{}' because it's a reserved argument.", a.name),
                p.start,
                p.end,
            ));
        }
    }
}
