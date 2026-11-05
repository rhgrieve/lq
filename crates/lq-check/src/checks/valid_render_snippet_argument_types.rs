//! `ValidRenderSnippetArgumentTypes`.

use super::shared_args::is_known_param_type;
use crate::api::*;
use crate::liquid_doc::arguments::{
    find_type_mismatch_params, liquid_doc_params, report_type_mismatches, snippet_name, with_type_mismatch_suggestions,
    DocParams,
};
use crate::liquid_doc::{argument_type_mismatch_message, check_argument_type, ArgumentTypeCheck};
use lq_syntax::{Node, RenderMarkup};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::RenderMarkup]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check))
}

struct Check;

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::RenderMarkup(markup) = node else { return };
        let Some(name) = snippet_name(node) else { return };
        let Some(params) = liquid_doc_params(cx, &format!("snippets/{name}.liquid")) else { return };
        report_alias_type(markup, &params, cx);
        report_type_mismatches(cx, &find_type_mismatch_params(&params, &markup.args), &params);
    }
}

/// `findAndReportAliasType`: `{% render 'card' with 123 as title %}`.
fn report_alias_type<'a>(markup: &RenderMarkup<'a>, params: &DocParams<'_>, cx: &LiquidCx<'a, '_>) {
    let (Some(Node::RenderAliasExpression(alias)), Some(Node::RenderVariableExpression(variable))) =
        (&markup.alias, &markup.variable)
    else {
        return;
    };
    let Some(expected) = params.get(alias.value).and_then(|p| p.ty.as_deref()).filter(|t| !t.is_empty()) else {
        return;
    };
    let argument = &variable.name;
    if !is_alias_mismatch(expected, check_argument_type(expected, argument), cx) {
        return;
    }
    let p = argument.position();
    let message = argument_type_mismatch_message(alias.value, expected, argument, cx.source());
    cx.report(with_type_mismatch_suggestions(LiquidProblem::new(message, p.start, p.end), expected, p.start, p.end));
}

/// `isAliasMismatch`. The docset lookup uses the declared spelling, as `ValidDocParamTypes` does.
fn is_alias_mismatch(expected: &str, check: ArgumentTypeCheck, cx: &LiquidCx<'_, '_>) -> bool {
    match check {
        ArgumentTypeCheck::Incompatible => true,
        ArgumentTypeCheck::NamedType(_) => cx.docset().is_none_or(|docset| is_known_param_type(docset, expected)),
        ArgumentTypeCheck::Compatible | ArgumentTypeCheck::Unchecked => false,
    }
}
