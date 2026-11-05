//! `ValidVisibleIf`, on Liquid files.

use super::shared_schema::*;
use crate::api::*;
use crate::to_schema::ThemeSchemaType;
use crate::visible_if::{match_index_units, global_settings, settings_scope, validate_lookup, variable_lookups_in_expression, ExpressionLookups, OFFSET_ADJUST};
use lq_json::ast::{node_at_path, PathKey};
use lq_json::{JsonNode, Object, Value};
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag, NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    // `getSchema` is undefined elsewhere, and nothing else can report.
    let in_scope = crate::to_schema::is_block(cx.uri()) || crate::to_schema::is_section(cx.uri());
    in_scope.then(|| Box::new(Check { assigned: Vec::new(), schema_offset: None }) as Box<dyn LiquidCheck<'a> + 'a>)
}

struct Check<'a> {
    assigned: Vec<&'a str>,
    schema_offset: Option<u32>,
}

fn run(assigned: &[&str], offset: u32, cx: &LiquidCx<'_, '_>) -> Js<()> {
    let Some(parts) = valid_parts(cx) else { return Ok(()) };
    let Some(settings) = nullish(parts.valid.get("settings")) else { return Ok(()) };
    let settings = array_of(settings)?;
    let mut any_visible_if = false;
    for setting in settings {
        if in_op(setting, "visible_if")? {
            any_visible_if = true;
            break;
        }
    }
    if !any_visible_if {
        return Ok(());
    }

    let mut vars = Object::new();
    for name in assigned {
        // `vars.__proto__ = true` sets nothing.
        if *name != "__proto__" {
            vars.insert(*name, Value::Bool(true));
        }
    }
    vars.insert("settings", settings_scope(global_settings(cx).iter().map(|s| js_string(Some(s)))));
    let mut current = Vec::with_capacity(settings.len());
    for setting in settings {
        current.push(js_string(prop(setting, "id")?));
    }
    let current = settings_scope(current.into_iter());
    let is_section = parts.schema.ty == ThemeSchemaType::Section;
    let is_block = parts.schema.ty == ThemeSchemaType::Block;
    if is_section || is_block {
        let mut scope = Object::new();
        scope.insert("settings", current);
        vars.insert(if is_section { "section" } else { "block" }, Value::Object(scope));
    }
    let vars = Value::Object(vars);

    for (i, setting) in settings.iter().enumerate() {
        if !in_op(setting, "visible_if")? {
            continue;
        }
        let Some(expression) = setting.get("visible_if").and_then(Value::as_str) else { continue };
        let node = node_at_path(parts.ast, &[PathKey::Str("settings"), PathKey::Index(i), PathKey::Str("visible_if")]);
        let loc_of = |n: Option<&JsonNode<'_>>| n.map(JsonNode::loc).ok_or_else(|| type_error("Cannot read properties of undefined (reading 'loc')"));
        let lookups = match variable_lookups_in_expression(expression) {
            ExpressionLookups::None => continue,
            ExpressionLookups::Warning(warning) => {
                let loc = loc_of(node)?;
                cx.report(LiquidProblem::new(warning, offset + loc.start, offset + loc.end));
                continue;
            }
            ExpressionLookups::Lookups(lookups) => lookups,
        };
        for lookup in &lookups {
            let name = lookup.name.as_deref();
            let message = if name == Some("section") && is_block {
                None
            } else if name == Some("section") && !is_section {
                Some("Invalid visible_if: can't refer to \"section\" when not in a section or block file.".to_string())
            } else if name == Some("block") && !is_block {
                Some("Invalid visible_if: can't refer to \"block\" when not in a block file.".to_string())
            } else {
                validate_lookup(lookup, &vars, cx.divergence(Divergence::PrototypeLookup)).map_err(|e| crate::visible_if::lookup_error(&e))?
            };
            if let Some(message) = message {
                let start = offset + loc_of(node)?.start;
                let index = if cx.divergence(Divergence::VisibleIfMatchIndex) { match_index_units(expression) } else { 0 };
                let adjust = |units: u32| advance_utf16(cx.source(), start, (units as i64 + i64::from(index) + OFFSET_ADJUST + 1) as u32);
                cx.report(LiquidProblem::new(message, adjust(lookup.start_units), adjust(lookup.end_units)));
            }
        }
    }
    Ok(())
}

impl<'a> LiquidCheck<'a> for Check<'a> {
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], _cx: &LiquidCx<'a, '_>) {
        match node {
            Node::LiquidTag(tag) if tag.name == "assign" => {
                if let lq_syntax::Markup::Node(markup) = &tag.markup {
                    if let Node::AssignMarkup(m) = &**markup {
                        self.assigned.push(m.name);
                    }
                }
            }
            Node::LiquidRawTag(_) => {
                if let Some(tag) = schema_raw_tag(node) {
                    self.schema_offset = Some(tag.block_start_position.end);
                }
            }
            _ => {}
        }
    }

    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {
        let Some(offset) = self.schema_offset else { return };
        if let Err(e) = run(&self.assigned, offset, cx) {
            cx.fail(e);
        }
    }
}
