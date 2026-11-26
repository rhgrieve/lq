//! `ValidSettingsKey`, on JSON templates: the settings a template gives a section or block
//! must exist in that section's or block's schema (theme-tools f3d24bf8).

use super::shared_schema::*;
use super::valid_settings_key::validate_settings_key;
use crate::api::*;
use crate::to_schema::parsed_json;
use lq_json::{JsonNode, Value};

pub const IMPL: CheckImpl = CheckImpl::Json { create, enter: JsonMask::NONE, exit: JsonMask::NONE };

fn create<'a, 'f>(cx: &JsonCx<'a, 'f>) -> Option<Box<dyn JsonCheck<'a> + 'a>> {
    cx.to_relative_path(cx.uri()).starts_with("templates/").then(|| Box::new(Check) as Box<dyn JsonCheck<'a> + 'a>)
}

struct Check;

/// `isPropertyNode(node)`: an object or array.
fn is_container(v: &Value) -> bool {
    matches!(v, Value::Object(_) | Value::Array(_))
}

fn path(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

impl<'a> JsonCheck<'a> for Check {
    fn on_code_path_end(&mut self, cx: &JsonCx<'a, '_>) {
        let Ok(ast) = cx.file().json_ast() else { return };
        let Some(parsed) = parsed_json(cx.source()) else { return };
        let Some(sections) = parsed.get("sections").filter(|s| s.is_truthy()) else { return };
        for (key, section) in entries(sections) {
            if !is_container(section) || !matches!(in_op(section, "type"), Ok(true)) {
                continue;
            }
            let Some(ty) = section.get("type").and_then(Value::as_str) else { continue };
            let Some(valid) = cx.section_schema(ty).and_then(|s| s.valid_value()) else { continue };
            let schema_path = format!("'sections/{ty}.liquid'");
            if matches!(in_op(section, "settings"), Ok(true)) {
                let node = node_at(ast, &path(&["sections", &key, "settings"]));
                if let Err(e) = validate_settings_key(cx, 0, node, valid.get("settings"), None, &schema_path) {
                    cx.fail(e);
                    return;
                }
            }
            if let Some(blocks) = section.get("blocks").filter(|b| is_container(b)) {
                if let Err(e) = template_blocks(cx, ast, blocks, valid, path(&["sections", &key, "blocks"]), &schema_path) {
                    cx.fail(e);
                    return;
                }
            }
        }
    }
}

/// `validateTemplateBlocksSettings(context, ast, blocks, parentSchema, currentPath, parentSchemaPath)`.
/// A local block's settings are in its parent's schema; a theme block's in its own file.
fn template_blocks(cx: &JsonCx<'_, '_>, ast: &JsonNode<'_>, blocks: &Value, parent: &Value, current: Vec<String>, parent_path: &str) -> Js<()> {
    for (key, block) in entries(blocks) {
        if !is_container(block) || !in_op(block, "type")? {
            continue;
        }
        let mut block_path = current.clone();
        block_path.push(key.into_owned());
        let settings_at = |p: &[String]| {
            let mut p = p.to_vec();
            p.push("settings".into());
            node_at(ast, &p)
        };
        let nested_at = |p: &[String]| {
            let mut p = p.to_vec();
            p.push("blocks".into());
            p
        };
        let ty = block.get("type");
        let local = parent.get("blocks").and_then(Value::as_array).and_then(|bs| {
            bs.iter().find(|b| b.get("type") == ty && matches!(b, Value::Object(o) if o.contains_key("name")))
        });
        if let Some(local) = local {
            validate_settings_key(cx, 0, settings_at(&block_path), local.get("settings"), None, parent_path)?;
            if let Some(nested) = block.get("blocks").filter(|b| is_container(b)) {
                template_blocks(cx, ast, nested, local, nested_at(&block_path), parent_path)?;
            }
            continue;
        }
        let Some(name) = ty.and_then(Value::as_str) else { continue };
        let Some(valid) = cx.block_schema(name).and_then(|s| s.valid_value()) else { continue };
        validate_settings_key(cx, 0, settings_at(&block_path), valid.get("settings"), Some(block), "schema")?;
        if let Some(nested) = block.get("blocks").filter(|b| is_container(b)) {
            template_blocks(cx, ast, nested, valid, nested_at(&block_path), &format!("'blocks/{name}.liquid'"))?;
        }
    }
    Ok(())
}
