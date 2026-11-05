//! `ValidSettingsKey`.

use super::shared_schema::*;
use crate::api::*;
use crate::to_schema::ThemeSchemaType;
use crate::utils::block::{get_blocks, BlockWithPath};
use lq_json::ast::{node_at_path, PathKey};
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
        let keys = Keys { cx, offset: tag.block_start_position.end };
        if let Err(e) = keys.run(&parts) {
            cx.fail(e);
        }
    }
}

struct Keys<'c, 'a, 'f> {
    cx: &'c LiquidCx<'a, 'f>,
    offset: u32,
}

impl Keys<'_, '_, '_> {
    fn run(&self, parts: &SchemaParts<'_>) -> Js<()> {
        use PathKey::{Index, Str};
        let (valid, ast) = (parts.valid, parts.ast);
        let found = get_blocks(valid);
        let schema_settings = valid.get("settings");

        // Presets' settings against the schema's.
        if let Some(presets) = valid.get("presets").and_then(Value::as_array) {
            for i in 0..presets.len() {
                let node = node_at_path(ast, &[Str("presets"), Index(i), Str("settings")]);
                self.validate_settings_key(node, schema_settings, None)?;
            }
        }

        // The default's settings, and its blocks' against their definitions.
        if parts.schema.ty == ThemeSchemaType::Section {
            if let Some(default) = valid.get("default").filter(|d| d.is_truthy()) {
                let node = node_at_path(ast, &["default", "settings"]);
                self.validate_settings_key(node, schema_settings, None)?;
                if let Some(blocks) = nullish(default.get("blocks")) {
                    let mut first_error = None;
                    for (i, block) in array_of(blocks)?.iter().enumerate() {
                        let node = node_at_path(ast, &[Str("default"), Str("blocks"), Index(i), Str("settings")]);
                        if let Err(e) = self.validate_referenced_block(node, &found.root_level_local_blocks, block) {
                            first_error.get_or_insert(e);
                        }
                    }
                    if let Some(e) = first_error {
                        return Err(e);
                    }
                }
            }
        }

        // Preset blocks' settings against their definitions.
        for blocks in found.preset_level_blocks.values() {
            let mut first_error = None;
            for BlockWithPath { node: block, path } in blocks {
                let mut keys: Vec<&str> = path[..path.len() - 1].iter().map(String::as_str).collect();
                keys.push("settings");
                let node = node_at_path(ast, &keys);
                if let Err(e) = self.validate_referenced_block(node, &found.root_level_local_blocks, block) {
                    first_error.get_or_insert(e);
                }
            }
            if let Some(e) = first_error {
                return Err(e);
            }
        }
        Ok(())
    }

    fn validate_referenced_block(&self, settings_node: Option<&JsonNode<'_>>, local_blocks: &[BlockWithPath<'_>], block: &Value) -> Js<()> {
        let block_type = prop(block, "type")?;
        if !local_blocks.is_empty() {
            let Some(local) = local_blocks.iter().find(|l| strict_eq(l.node.get("type"), block_type)) else { return Ok(()) };
            return self.validate_settings_key(settings_node, local.node.get("settings"), None);
        }
        let Some(schema) = block_type.and_then(Value::as_str).and_then(|n| self.cx.block_schema(n)) else { return Ok(()) };
        let Some(valid) = schema.valid_value() else { return Ok(()) };
        self.validate_settings_key(settings_node, valid.get("settings"), Some(block))
    }

    fn validate_settings_key(&self, settings_node: Option<&JsonNode<'_>>, valid_settings: Option<&Value>, block: Option<&Value>) -> Js<()> {
        let Some(node @ JsonNode::Object { .. }) = settings_node else { return Ok(()) };
        for setting in node.children() {
            let JsonNode::Property { key, .. } = setting else { continue };
            let name = setting.property_key().unwrap_or("");
            let exists = match nullish(valid_settings) {
                Some(settings) => array_of(settings)?.iter().any(|s| s.get("id").and_then(Value::as_str) == Some(name)),
                None => false,
            };
            if !exists {
                let message = match block {
                    Some(b) => format!("Setting '{name}' does not exist in 'blocks/{}.liquid'.", js_string(b.get("type"))),
                    None => format!("Setting '{name}' does not exist in schema."),
                };
                let loc = key.loc();
                self.cx.report(LiquidProblem::new(message, self.offset + loc.start, self.offset + loc.end));
            }
        }
        Ok(())
    }
}
