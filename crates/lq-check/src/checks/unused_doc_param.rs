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
        let optional = |node: &Node<'_>| matches!(node, Node::LiquidDocParamNode(p) if !p.required);
        let used_optional = self.defined.iter().any(|&(v, node)| optional(node) && self.used.contains(v));
        let provided = if used_optional { provided_params(cx) } else { None };
        for &(variable, node) in &self.defined {
            let p = node.position();
            if !self.used.contains(variable) {
                cx.report(
                    LiquidProblem::new(format!("The parameter '{variable}' is defined but not used in this file."), p.start, p.end)
                        .suggest(format!("Remove unused parameter '{variable}'"), |c| c.remove(p.start, p.end)),
                );
            } else if optional(node) && provided.as_ref().is_some_and(|set| !set.contains(variable)) {
                cx.report(LiquidProblem::new(format!("The parameter '{variable}' is never passed to this snippet."), p.start, p.end));
            }
        }
    }
}

/// `getProvidedParams(context)`: the parameters the snippet's `render` callers pass to it
/// (theme-tools c374706b). `None` when that can't be known: the snippet has no callers, a
/// caller can't be read, or a caller uses `include`, whose snippet inherits its variables.
fn provided_params(cx: &LiquidCx<'_, '_>) -> Option<HashSet<String>> {
    if !cx.has_references() || !crate::to_schema::is_snippet(cx.uri()) {
        return None;
    }
    let relative = cx.to_relative_path(cx.uri());
    let snippet = relative.strip_prefix("snippets/").unwrap_or(&relative);
    let snippet = snippet.strip_suffix(".liquid").unwrap_or(snippet);
    let mut sources: Vec<String> = Vec::new();
    for r in cx.references(cx.uri()) {
        if r.ty == crate::graph::ReferenceType::Direct && !sources.contains(&r.source.uri) {
            sources.push(r.source.uri);
        }
    }
    // A snippet without callers is OrphanedSnippet's to report.
    if sources.is_empty() {
        return None;
    }
    let mut provided = HashSet::new();
    for u in &sources {
        let text = cx.fs().read_file(u).ok()?;
        let file = crate::source::to_source_code(u, &text);
        let root = file.liquid_ast().ok()?;
        let mut include = false;
        crate::visitor::visit(root, &mut |node, ancestors| {
            let Node::RenderMarkup(m) = node else { return };
            if crate::liquid_doc::arguments::snippet_name(node) != Some(snippet) {
                return;
            }
            if matches!(ancestors.last(), Some(Node::LiquidTag(t)) if t.name == "include") {
                include = true;
                return;
            }
            for arg in &m.args {
                if let Node::NamedArgument(a) = arg {
                    provided.insert(a.name.to_string());
                }
            }
            if m.variable.is_some() {
                let alias = match &m.alias {
                    Some(Node::RenderAliasExpression(a)) => a.value,
                    _ => snippet,
                };
                provided.insert(alias.to_string());
            }
        });
        if include {
            return None;
        }
    }
    Some(provided)
}
