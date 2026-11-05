//! `ParserBlockingScript`.

use crate::api::*;
use crate::check_utils::{has_attribute_value_of, is_attr, valued_attribute};
use lq_syntax::{LiquidVariable, Node};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidFilter, NodeKind::HtmlRawNode]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(ParserBlockingScript))
}

struct ParserBlockingScript;

fn suggestion_message(attr: &str) -> String {
    format!("Use an HTML script tag with the {attr} attribute instead")
}

fn filter_replacement(attr: &str, node: &Node<'_>, parent: &LiquidVariable<'_>, source: &str) -> String {
    let filter_count = parent.filters.len();
    let end = if filter_count >= 2 { parent.filters[filter_count - 2].position().end } else { node.position().start };
    let expression = lq_syntax::js::slice(source, parent.expression.position().start, end);
    format!("<script src=\"{{{{ {expression} }}}}\" {attr}></script>")
}

impl<'a> LiquidCheck<'a> for ParserBlockingScript {
    fn enter(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        match node {
            Node::LiquidFilter(filter) if filter.name == "script_tag" => {
                let p = node.position();
                let source = cx.source();
                let offset = lq_syntax::js::slice(source, p.start, p.end).find("script_tag").unwrap_or(0) as u32;
                let mut problem = LiquidProblem::new(
                    "The script_tag filter is parser-blocking. Use a <script> tag with async or defer for better performance",
                    p.start + offset,
                    p.end,
                );
                if let [.., grandparent @ Node::LiquidVariableOutput(_), Node::LiquidVariable(parent)] = ancestors {
                    if parent.filters.last().is_some_and(|last| std::ptr::eq(last, node)) {
                        let span = grandparent.position();
                        for attr in ["defer", "async"] {
                            let text = filter_replacement(attr, node, parent, source);
                            problem = problem
                                .suggest(suggestion_message(attr), |c| c.replace(span.start, span.end, text));
                        }
                    }
                }
                cx.report(problem);
            }
            Node::HtmlRawNode(element) if element.name == "script" => {
                let attributes = &element.attributes;
                if !attributes.iter().any(|a| valued_attribute(a).is_some() && is_attr(a, "src")) {
                    return;
                }
                if attributes.iter().any(|a| is_attr(a, "async") || is_attr(a, "defer")) {
                    return;
                }
                let is_type_module = attributes.iter().any(|a| {
                    is_attr(a, "type")
                        && valued_attribute(a).is_some_and(|v| {
                            has_attribute_value_of(v, "module") || has_attribute_value_of(v, "importmap")
                        })
                });
                if is_type_module {
                    return;
                }
                let p = node.position();
                let insert_at = element.block_start_position.end - 1;
                let mut problem = LiquidProblem::new(
                    "Avoid parser blocking scripts by adding `defer` or `async` on this tag",
                    p.start,
                    p.end,
                );
                for attr in ["defer", "async"] {
                    problem = problem.suggest(suggestion_message(attr), |c| c.insert(insert_at, format!(" {attr}")));
                }
                cx.report(problem);
            }
            _ => {}
        }
    }
}
