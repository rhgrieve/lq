//! `UnusedDocParam`.

use crate::api::*;
use crate::check_utils::is_loop_scoped_variable;
use lq_syntax::Node;
use std::collections::HashSet;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidDocParamNode, NodeKind::VariableLookup]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    // Only `@param` in a doc tag makes a `LiquidDocParamNode`: without one, nothing
    // can be reported and the lookups needn't be collected.
    if !cx.source().contains("@param") {
        return None;
    }
    Some(Box::<Check<'a>>::default())
}

#[derive(Default)]
struct Check<'a> {
    /// `Map<string, node>`: a redefinition replaces the node but keeps the slot.
    defined: Vec<(&'a str, &'a Node<'a>)>,
    used: HashSet<&'a str>,
}

impl<'a> LiquidCheck<'a> for Check<'a> {
    fn enter(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], _cx: &LiquidCx<'a, '_>) {
        match node {
            Node::LiquidDocParamNode(param) => {
                let Node::TextNode(name) = &param.param_name else { return };
                let name: &'a str = &name.value;
                match self.defined.iter_mut().find(|(n, _)| *n == name) {
                    Some(slot) => slot.1 = node,
                    None => self.defined.push((name, node)),
                }
            }
            Node::VariableLookup(lookup) => {
                let Some(name) = lookup.name.filter(|n| !n.is_empty()) else { return };
                if !is_loop_scoped_variable(name, ancestors) {
                    self.used.insert(name);
                }
            }
            _ => {}
        }
    }

    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {
        for &(variable, node) in &self.defined {
            if self.used.contains(variable) {
                continue;
            }
            let p = node.position();
            cx.report(
                LiquidProblem::new(format!("The parameter '{variable}' is defined but not used in this file."), p.start, p.end)
                    .suggest(format!("Remove unused parameter '{variable}'"), |c| c.remove(p.start, p.end)),
            );
        }
    }
}
