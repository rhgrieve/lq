//! `UnrecognizedRenderSnippetArguments`.

use super::shared_args::named;
use crate::api::*;
use crate::liquid_doc::arguments::{liquid_doc_params, report_unknown_arguments, snippet_name};
use lq_syntax::Node;

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
        if let (Some(alias_node @ Node::RenderAliasExpression(alias)), Some(variable)) = (&markup.alias, &markup.variable) {
            if !params.has(alias.value) {
                let (start, end) = (variable.position().start, alias_node.position().end);
                cx.report(
                    LiquidProblem::new(
                        format!("Unknown argument '{}' in render tag for snippet '{name}'.", alias.value),
                        start,
                        end,
                    )
                    .hints_if(cx.wants_hints(), |p| p.label("unknown argument"))
                    .suggest(format!("Remove '{}'", alias.value), |c| c.remove(start - 1, end)),
                );
            }
        }
        let unknown: Vec<_> =
            markup.args.iter().filter(|arg| named(arg).is_some_and(|a| !params.has(&a.name))).collect();
        report_unknown_arguments(cx, node, &unknown, name, &params);
    }
}
