//! `UnusedAssign`: a stateful Liquid check, walking the AST.

use crate::api::*;
use crate::check_utils::is_within_unparsed_raw_tag;
use crate::visible_if::{variable_lookups_in_expression, ExpressionLookups};
use lq_json::Value;
use lq_syntax::{LiquidTag, Markup, Node};
use std::collections::{HashMap, HashSet};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag, NodeKind::VariableLookup, NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::<UnusedAssign<'a>>::default())
}

#[derive(Default)]
struct UnusedAssign<'a> {
    /// `Map<string, node>`: insertion order for reporting, a later assign of the
    /// same name replaces the node but keeps the slot.
    assigned: Vec<(&'a str, &'a Node<'a>)>,
    assigned_index: HashMap<&'a str, usize>,
    /// Names from the file itself.
    used: HashSet<&'a str>,
    /// Names from `visible_if` expressions, parsed out of schema strings.
    used_owned: HashSet<String>,
}

fn is_assign(tag: &LiquidTag<'_>) -> bool {
    tag.name == "assign" && !matches!(tag.markup, Markup::Str(_))
}

fn is_capture(tag: &LiquidTag<'_>) -> bool {
    tag.name == "capture" && !matches!(tag.markup, Markup::Str(_))
}

/// The variable a tag assigns: `node.markup.name`.
fn assigned_name<'a>(tag: &'a LiquidTag<'a>) -> Option<&'a str> {
    let Markup::Node(markup) = &tag.markup else { return None };
    match &**markup {
        Node::AssignMarkup(m) => Some(m.name),
        // `{% capture x %}`'s markup is a VariableLookup; a nameless one is falsy.
        Node::VariableLookup(l) => l.name.filter(|n| !n.is_empty()),
        _ => None,
    }
}

impl<'a> LiquidCheck<'a> for UnusedAssign<'a> {
    fn enter(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        match node {
            Node::LiquidTag(tag) => {
                if is_within_unparsed_raw_tag(ancestors) || !(is_assign(tag) || is_capture(tag)) {
                    return;
                }
                let Some(name) = assigned_name(tag) else { return };
                match self.assigned_index.get(name) {
                    Some(&i) => self.assigned[i].1 = node,
                    None => {
                        self.assigned_index.insert(name, self.assigned.len());
                        self.assigned.push((name, node));
                    }
                }
            }
            Node::VariableLookup(lookup) => {
                if is_within_unparsed_raw_tag(ancestors) {
                    return;
                }
                if let Some(Node::LiquidTag(parent)) = ancestors.last() {
                    if is_capture(parent) {
                        return;
                    }
                }
                if let Some(name) = lookup.name {
                    self.used.insert(name);
                }
            }
            Node::LiquidRawTag(tag) => {
                let Node::RawMarkup(body) = &tag.body else { return };
                if tag.name != "schema" || body.kind != lq_syntax::RawMarkupKind::Json {
                    return;
                }
                let Some(valid) = cx.schema().and_then(|s| s.valid_value()) else { return };
                let Some(settings) = valid.get("settings").filter(|s| s.is_truthy()) else { return };
                for setting in settings.as_array().unwrap_or(&[]) {
                    let Some(Value::String(visible_if)) = setting.get("visible_if") else { continue };
                    let ExpressionLookups::Lookups(lookups) = variable_lookups_in_expression(visible_if) else {
                        continue;
                    };
                    for lookup in lookups {
                        if let Some(name) = lookup.name.filter(|n| !n.is_empty()) {
                            self.used_owned.insert(name);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {
        for &(variable, node) in &self.assigned {
            if self.used.contains(variable) || self.used_owned.contains(variable) || variable.starts_with('_') {
                continue;
            }
            let Node::LiquidTag(tag) = node else { continue };
            let position = node.position();
            let span = if is_capture(tag) { tag.block_start_position } else { position };
            cx.report(
                LiquidProblem::new(format!("The variable '{variable}' is assigned but not used"), span.start, span.end)
                    .suggest(format!("Remove the unused variable '{variable}'"), |c| {
                        c.remove(position.start, position.end);
                    }),
            );
        }
    }
}
