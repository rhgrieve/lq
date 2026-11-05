//! `DeprecatedFontsOnSectionsAndBlocks`.

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
        let walk = Walk { cx, schema: parts.valid, ast: parts.ast, offset: tag.block_start_position.end };
        if let Err(e) = walk.run() {
            cx.fail(e);
        }
    }
}

struct Walk<'c, 'a, 'f> {
    cx: &'c LiquidCx<'a, 'f>,
    schema: &'a Value,
    ast: &'a JsonNode<'a>,
    offset: u32,
}

fn path_of(base: &[String], more: &[&str]) -> Vec<String> {
    base.iter().cloned().chain(more.iter().map(|s| s.to_string())).collect()
}

/// `isFontPickerType(settings ?? [], settingKey)`.
fn is_font_picker_type(settings: Option<&Value>, key: &str) -> Js<bool> {
    let Some(settings) = nullish(settings) else { return Ok(false) };
    for setting in array_of(settings)? {
        if prop(setting, "id")?.and_then(Value::as_str) == Some(key) && prop(setting, "type")?.and_then(Value::as_str) == Some("font_picker") {
            return Ok(true);
        }
    }
    Ok(false)
}

impl Walk<'_, '_, '_> {
    fn report(&self, path: &[String], message: String) -> Js<()> {
        let loc = loc_at(self.ast, path)?;
        self.cx.report(LiquidProblem::new(message, self.offset + loc.start, self.offset + loc.end));
        Ok(())
    }

    fn run(&self) -> Js<()> {
        let schema = self.schema;
        // 1st: schema settings
        if let Some(settings) = prop(schema, "settings")?.filter(|s| s.is_truthy()) {
            self.check_settings(settings, &["settings".to_string()])?;
        }
        // 2nd: local blocks settings
        if let Some(blocks) = prop(schema, "blocks")?.filter(|b| b.is_truthy()) {
            for (i, block) in array_of(blocks)?.iter().enumerate() {
                if in_op(block, "settings")? {
                    if let Some(settings) = block.get("settings").filter(|s| s.is_truthy()) {
                        self.check_settings(settings, &["blocks".to_string(), i.to_string(), "settings".to_string()])?;
                    }
                }
            }
        }
        // 3rd: presets
        if let Some(presets) = prop(schema, "presets")?.filter(|p| p.is_truthy()) {
            for (i, preset) in array_of(presets)?.iter().enumerate() {
                let path = ["presets".to_string(), i.to_string()];
                let settings = nullish(prop(preset, "settings")?);
                let blocks = if in_op(preset, "blocks")? { preset.get("blocks") } else { None };
                self.check_settings_and_blocks(settings, blocks, &path)?;
            }
        }
        // 4th: default
        if let Some(default) = schema.get("default") {
            if matches!(default, Value::Object(_) | Value::Array(_)) {
                let settings = nullish(prop(default, "settings")?);
                let blocks = if in_op(default, "blocks")? { default.get("blocks") } else { None };
                self.check_settings_and_blocks(settings, blocks, &["default".to_string()])?;
            }
        }
        Ok(())
    }

    fn check_settings(&self, settings: &Value, path: &[String]) -> Js<()> {
        for (i, setting) in array_of(settings)?.iter().enumerate() {
            if prop(setting, "type")?.and_then(Value::as_str) != Some("font_picker") {
                continue;
            }
            let Some(default) = setting.get("default").and_then(Value::as_str) else { continue };
            if is_deprecated_font(default) {
                let id = js_string(setting.get("id"));
                let index = i.to_string();
                self.report(
                    &path_of(path, &[&index, "default"]),
                    format!("setting '{id}' is using deprecated font '{default}'"),
                )?;
            }
        }
        Ok(())
    }

    fn check_settings_and_blocks(&self, settings: Option<&Value>, blocks: Option<&Value>, path: &[String]) -> Js<()> {
        if let Some(settings) = settings {
            for (key, value) in entries(settings) {
                if is_font_picker_type(self.schema.get("settings"), &key)? {
                    if let Some(font) = value.as_str().filter(|f| is_deprecated_font(f)) {
                        self.report(
                            &path_of(path, &["settings", &key]),
                            format!("setting '{key}' is using deprecated font '{font}'"),
                        )?;
                    }
                }
            }
        }
        if let Some(blocks) = blocks.filter(|b| b.is_truthy()) {
            self.check_blocks(blocks, path)?;
        }
        Ok(())
    }

    fn check_blocks(&self, blocks: &Value, node_path: &[String]) -> Js<()> {
        for (key, block) in entries(blocks) {
            let path = path_of(node_path, &["blocks", &key]);
            let block_type = prop(block, "type")?;
            // Local blocks have their settings in this schema, theme blocks in their file.
            let mut definition: Option<&Value> = None;
            if let Some(schema_blocks) = nullish(self.schema.get("blocks")) {
                for schema_block in array_of(schema_blocks)? {
                    if strict_eq(prop(schema_block, "type")?, block_type) && in_op(schema_block, "name")? {
                        definition = Some(schema_block);
                    }
                }
            }
            let definition = match definition {
                Some(d) => d,
                None => {
                    let Some(file_schema) = block_type.and_then(Value::as_str).and_then(|n| self.cx.block_schema(n)) else { continue };
                    let Some(valid) = file_schema.valid_value() else { continue };
                    valid
                }
            };
            if let Some(settings) = nullish(block.get("settings")) {
                for (setting_key, value) in entries(settings) {
                    let Some(font) = value.as_str().filter(|f| is_deprecated_font(f)) else { continue };
                    if is_font_picker_type(prop(definition, "settings")?, &setting_key)? {
                        self.report(
                            &path_of(&path, &["settings", &setting_key]),
                            format!("setting '{setting_key}' is using deprecated font '{font}'"),
                        )?;
                    }
                }
            }
            if in_op(block, "blocks")? {
                if let Some(nested) = block.get("blocks").filter(|b| b.is_truthy()) {
                    self.check_blocks(nested, &path)?;
                }
            }
        }
        Ok(())
    }
}
