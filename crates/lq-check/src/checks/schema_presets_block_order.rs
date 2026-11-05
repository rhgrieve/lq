//! `SchemaPresetsBlockOrder`.

use super::shared_schema::*;
use crate::api::*;
use lq_json::{JsonNode, Value};
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check))
}

struct Check;

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Some(tag) = schema_raw_tag(node) else { return };
        let Some(parts) = valid_parts(cx) else { return };
        let offset = tag.block_start_position.end;
        if let Err(e) = check_presets(parts.valid, parts.ast, offset, cx) {
            cx.fail(e);
        }
    }
}

fn check_presets(valid: &Value, ast: &JsonNode<'_>, offset: u32, cx: &LiquidCx<'_, '_>) -> Js<()> {
    let Some(presets) = valid.get("presets").filter(|p| p.is_truthy()) else { return Ok(()) };
    for (index, preset) in array_of(presets)?.iter().enumerate() {
        if in_op(preset, "blocks")? && preset.get("blocks").is_some_and(Value::is_truthy) {
            let mut path = vec!["presets".to_string(), index.to_string()];
            check_block_order(preset, cx, offset, ast, &mut path)?;
        }
    }
    Ok(())
}

/// Upstream appends to the shared `ast_path` when it recurses and never pops, so a sibling
/// after a block with nested blocks sees a wrong path: a report lands on the wrong node, or
/// `nodeAtPath` finds nothing and the check throws. [`Divergence::PresetsBlockOrderPath`]
/// pops; `--upstream-bugs` keeps it.
fn check_block_order(node: &Value, cx: &LiquidCx<'_, '_>, offset: u32, ast: &JsonNode<'_>, ast_path: &mut Vec<String>) -> Js<()> {
    if !in_op(node, "blocks")? {
        return Ok(());
    }
    let Some(blocks) = node.get("blocks").filter(|b| matches!(b, Value::Object(_) | Value::Array(_))) else {
        return Ok(());
    };
    let block_order = if in_op(node, "block_order")? { node.get("block_order") } else { None };

    // check 1: is block_order missing?
    if !block_order.is_some_and(Value::is_truthy) && should_have_block_order(blocks)? {
        report(cx, offset, ast, ast_path, "block_order is missing".to_string(), false)?;
    }

    // check 2: are items in block_order present in blocks?
    let order: Option<&[Value]> = match nullish(block_order) {
        Some(order) => Some(array_of(order)?),
        None => None,
    };
    if let Some(order) = order {
        for (index, block_id) in order.iter().enumerate() {
            let id = js_string(Some(block_id));
            let present = match blocks {
                Value::Object(o) => o.get(&id).is_some_and(Value::is_truthy),
                _ => true,
            };
            if !present {
                let mut path = ast_path.clone();
                path.extend(["block_order".to_string(), index.to_string()]);
                report(cx, offset, ast, &path, format!("block '{id}' is missing from 'blocks'"), true)?;
            }
        }
    }

    // check 3: are items in blocks correctly in block_order?
    for (block_id, block) in entries(blocks) {
        let listed = order.map(|o| o.iter().any(|v| v.as_str() == Some(&block_id)));
        let is_static = prop(block, "static")?.is_some_and(Value::is_truthy);
        let message = match (is_static, listed) {
            (true, Some(true)) => Some(format!("static block '{block_id}' cannot be in the block_order")),
            (false, Some(false)) => Some(format!("block '{block_id}' is missing from the block_order")),
            _ => None,
        };
        if let Some(message) = message {
            let mut path = ast_path.clone();
            path.push("block_order".to_string());
            report(cx, offset, ast, &path, message, true)?;
        }
        if block.get("blocks").is_some_and(Value::is_truthy) {
            let depth = ast_path.len();
            ast_path.extend(["blocks".to_string(), block_id.to_string()]);
            check_block_order(block, cx, offset, ast, ast_path)?;
            if cx.divergence(Divergence::PresetsBlockOrderPath) {
                ast_path.truncate(depth);
            }
        }
    }
    Ok(())
}

fn should_have_block_order(blocks: &Value) -> Js<bool> {
    let Value::Object(o) = blocks else { return Ok(false) };
    for (_, block) in o.iter() {
        if prop(block, "static")?.and_then(Value::as_bool) != Some(true) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn report(cx: &LiquidCx<'_, '_>, offset: u32, ast: &JsonNode<'_>, path: &[String], message: String, full: bool) -> Js<()> {
    let loc = loc_at(ast, path)?;
    let start = if full { offset + loc.start } else { offset + loc.end - 1 };
    cx.report(LiquidProblem::new(message, start, offset + loc.end));
    Ok(())
}
