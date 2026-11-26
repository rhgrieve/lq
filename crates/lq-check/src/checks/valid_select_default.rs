//! `ValidSelectDefault`, on Liquid files: a select or radio setting's default, and the values
//! presets and section defaults give it, must be one of its options.

use super::shared_schema::*;
use super::shared_select::*;
use crate::api::*;
use crate::to_schema::ThemeSchemaType;
use crate::utils::block::{get_blocks, report_warning, BlockWithPath};
use lq_json::{JsonNode, Value};
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    // `getSchema` is undefined elsewhere, and nothing else can report.
    let in_scope = crate::to_schema::is_block(cx.uri()) || crate::to_schema::is_section(cx.uri());
    in_scope.then(|| Box::new(Check) as Box<dyn LiquidCheck<'a> + 'a>)
}

struct Check;

/// `referencedBlockSettings(context, ast, localBlocks, type)`: the settings of the block a preset
/// or section default uses. A section with local blocks can only use those.
fn referenced_block_settings<'a>(
    cx: &LiquidCx<'a, '_>,
    ast: &'a JsonNode<'a>,
    local: &[BlockWithPath<'_>],
    ty: Option<&Value>,
) -> Option<&'a JsonNode<'a>> {
    if !local.is_empty() {
        let block = local.iter().find(|b| b.node.get("type") == ty)?;
        return node_at(ast, &settings_path(&block.path));
    }
    let schema = cx.block_schema(ty?.as_str()?)?;
    schema.valid_value()?;
    let block_ast = schema.ast.as_ref().ok()?;
    node_at(block_ast, &["settings".to_string()])
}

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Some(tag) = schema_raw_tag(node) else { return };
        let Some(parts) = valid_parts(cx) else { return };
        let offset = tag.block_start_position.end;
        let ast = parts.ast;
        let mut report = |message: String, n: &JsonNode<'_>| report_warning(cx, message, offset, n);

        let settings = node_at(ast, &["settings".to_string()]);
        let found = get_blocks(parts.valid);

        // The defaults of the file's settings, and of its local blocks' settings.
        check_defaults(settings, &mut report);
        for b in &found.root_level_local_blocks {
            check_defaults(node_at(ast, &settings_path(&b.path)), &mut report);
        }

        // The values presets give the file's settings.
        let presets = parts.valid.get("presets").and_then(Value::as_array).map_or(0, <[Value]>::len);
        for i in 0..presets {
            check_values(node_at(ast, &["presets".to_string(), i.to_string(), "settings".to_string()]), settings, &mut report);
        }

        // The values presets and section defaults give their blocks' settings.
        let check_block = |path: &[String], ty: Option<&Value>, report: &mut dyn FnMut(String, &JsonNode<'_>)| {
            let block_settings = referenced_block_settings(cx, ast, &found.root_level_local_blocks, ty);
            check_values(node_at(ast, &settings_path(path)), block_settings, report);
        };
        for blocks in found.preset_level_blocks.values() {
            for b in blocks {
                check_block(&b.path, b.node.get("type"), &mut report);
            }
        }

        let default = parts.valid.get("default").filter(|d| d.is_truthy());
        if let (ThemeSchemaType::Section, Some(default)) = (parts.schema.ty, default) {
            check_values(node_at(ast, &["default".to_string(), "settings".to_string()]), settings, &mut report);
            let blocks = default.get("blocks").and_then(Value::as_array).unwrap_or(&[]);
            // `block.type` throws on a null block, before any of them is checked.
            if blocks.iter().any(|b| matches!(b, Value::Null)) {
                cx.fail(type_error("Cannot read properties of null (reading 'type')"));
                return;
            }
            for (i, block) in blocks.iter().enumerate() {
                let path = ["default".to_string(), "blocks".to_string(), i.to_string(), "type".to_string()];
                check_block(&path, block.get("type"), &mut report);
            }
        }
    }
}
