//! `ValidDocParamTypes`.

use super::shared_args::is_known_param_type;
use crate::api::*;
use crate::liquid_doc::parse_string_literals;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidDocParamNode]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check))
}

struct Check;

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidDocParamNode(param) = node else { return };
        let Some(param_type) = &param.param_type else { return };
        let Node::TextNode(text) = param_type else { return };
        let value: &str = &text.value;
        if is_supported_param_type(value, cx) {
            return;
        }
        let p = param_type.position();
        let (start, end) = (p.start - 1, p.end + 1);
        let (fix_start, fix_end) = param_type_removal_range(cx.source(), start, end);
        cx.report(
            LiquidProblem::new(format!("The parameter type '{value}' is not supported."), start, end)
                .suggest("Remove invalid parameter type", |c| c.replace(fix_start, fix_end, " ".to_string())),
        );
    }
}

fn is_supported_param_type(ty: &str, cx: &LiquidCx<'_, '_>) -> bool {
    if ty.contains(['\'', '"', '|']) {
        return parse_string_literals(ty).is_some();
    }
    // Named types come from the docset, so they can only be checked with one.
    cx.docset().is_none_or(|docset| is_known_param_type(docset, ty))
}

/// `paramTypeRemovalRange`: the type, its braces, and the spaces and tabs around them.
fn param_type_removal_range(source: &str, mut start: u32, mut end: u32) -> (u32, u32) {
    let bytes = source.as_bytes();
    let is_space_or_tab = |i: u32| matches!(bytes.get(i as usize), Some(b' ' | b'\t'));
    while start > 0 && is_space_or_tab(start - 1) {
        start -= 1;
    }
    while is_space_or_tab(end) {
        end += 1;
    }
    (start, end)
}
