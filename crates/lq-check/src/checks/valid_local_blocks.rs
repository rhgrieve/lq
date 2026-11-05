//! `ValidLocalBlocks`.

use super::shared_schema::*;
use crate::api::*;
use crate::to_schema::{is_block, is_section};
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

/// The result of this check's own `getBlocks`: `type` paths.
#[derive(Default)]
struct Locations {
    static_blocks: Vec<Vec<String>>,
    local_blocks: Vec<Vec<String>>,
    theme_blocks: Vec<Vec<String>>,
}

fn path_with(base: &[String], more: &[&str]) -> Vec<String> {
    base.iter().cloned().chain(more.iter().map(|s| s.to_string())).collect()
}

fn categorize(out: &mut Locations, block: &Value, path: &[String], in_preset: bool) -> Js<()> {
    if !block.is_truthy() {
        return Ok(());
    }
    let has_static = in_op(block, "static")?;
    let has_name = in_op(block, "name")?;
    if has_static {
        out.static_blocks.push(path_with(path, &["type"]));
    } else if has_name && !in_preset {
        out.local_blocks.push(path_with(path, &["type"]));
    } else if block.get("type").and_then(Value::as_str) != Some("@app") {
        out.theme_blocks.push(path_with(path, &["type"]));
    }
    if let Some(blocks) = block.get("blocks") {
        for (key, nested) in entries(blocks) {
            categorize(out, nested, &path_with(path, &["blocks", &key]), in_preset)?;
        }
    }
    Ok(())
}

fn get_blocks(valid: &Value) -> Js<Locations> {
    let mut out = Locations::default();
    if let Some(Value::Array(blocks)) = valid.get("blocks") {
        for (i, block) in blocks.iter().enumerate() {
            categorize(&mut out, block, &["blocks".to_string(), i.to_string()], false)?;
        }
    }
    if let Some(presets) = valid.get("presets").filter(|p| p.is_truthy()) {
        for (pi, preset) in array_of(presets)?.iter().enumerate() {
            if !in_op(preset, "blocks")? {
                continue;
            }
            let Some(blocks) = preset.get("blocks") else { continue };
            for (key, block) in entries(blocks) {
                let path = ["presets".to_string(), pi.to_string(), "blocks".to_string(), key.into_owned()];
                categorize(&mut out, block, &path, true)?;
            }
        }
    }
    Ok(out)
}

fn report_all(paths: &[Vec<String>], message: &str, offset: u32, ast: &JsonNode<'_>, cx: &LiquidCx<'_, '_>) -> Js<()> {
    for path in paths {
        let loc = loc_at(ast, path)?;
        cx.report(LiquidProblem::new(message, offset + loc.start, offset + loc.end));
    }
    Ok(())
}

fn check(valid: &Value, ast: &JsonNode<'_>, offset: u32, cx: &LiquidCx<'_, '_>) -> Js<()> {
    let found = get_blocks(valid)?;
    let has_root_level_theme_blocks = found.theme_blocks.iter().any(|p| p[0] == "blocks");
    if is_section(cx.uri()) {
        if !found.static_blocks.is_empty() && !found.local_blocks.is_empty() {
            report_all(&found.static_blocks, "Sections cannot use static theme blocks together with locally scoped blocks.", offset, ast, cx)?;
        }
        if has_root_level_theme_blocks && !found.local_blocks.is_empty() && !found.theme_blocks.is_empty() {
            report_all(&found.local_blocks, "Sections cannot use theme blocks together with locally scoped blocks.", offset, ast, cx)?;
        }
    }
    if is_block(cx.uri()) {
        report_all(&found.local_blocks, "Local scoped blocks are not supported in theme blocks.", offset, ast, cx)?;
    }
    Ok(())
}

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Some(tag) = schema_raw_tag(node) else { return };
        let Some(parts) = valid_parts(cx) else { return };
        if let Err(e) = check(parts.valid, parts.ast, tag.block_start_position.end, cx) {
            cx.fail(e);
        }
    }
}
