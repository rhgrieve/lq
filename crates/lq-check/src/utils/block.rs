//! `utils/block.ts`, over the dynamic `validSchema` value.

use crate::api::{Cx, LiquidCx, Problem};
use crate::checks::shared_schema::Sched;
use crate::fixes::Corrector;
use crate::to_schema::StaticBlockDef;
use lq_json::{JsonNode, Value};
use std::collections::BTreeMap;

/// `BlockDefNodeWithPath` / `PresetBlockNodeWithPath`.
#[derive(Clone, Debug)]
pub struct BlockWithPath<'v> {
    pub node: &'v Value,
    pub path: Vec<String>,
}

/// The result of `getBlocks(validSchema)`.
#[derive(Clone, Debug, Default)]
pub struct Blocks<'v> {
    pub root_level_theme_blocks: Vec<BlockWithPath<'v>>,
    pub root_level_local_blocks: Vec<BlockWithPath<'v>>,
    /// `presetLevelBlocks`, by depth.
    pub preset_level_blocks: BTreeMap<usize, Vec<BlockWithPath<'v>>>,
    pub default_level_blocks: Vec<BlockWithPath<'v>>,
    pub has_root_blocks_declaration: bool,
}

fn has(block: &Value, key: &str) -> bool {
    block.get(key).is_some()
}

fn block_type(block: &Value) -> &str {
    block.get("type").and_then(Value::as_str).unwrap_or("")
}

fn categorize_preset<'v>(out: &mut Blocks<'v>, block: &'v Value, path: Vec<String>, depth: usize) {
    if !block.is_truthy() {
        return;
    }
    let mut type_path = path.clone();
    type_path.push("type".into());
    out.preset_level_blocks.entry(depth).or_default().push(BlockWithPath { node: block, path: type_path });
    match block.get("blocks") {
        Some(Value::Array(a)) => {
            for (i, nested) in a.iter().enumerate() {
                let mut p = path.clone();
                p.push("blocks".into());
                p.push(i.to_string());
                categorize_preset(out, nested, p, depth + 1);
            }
        }
        Some(Value::Object(o)) => {
            for (key, nested) in o.iter() {
                let mut p = path.clone();
                p.push("blocks".into());
                p.push(key.to_string());
                categorize_preset(out, nested, p, depth + 1);
            }
        }
        _ => {}
    }
}

/// `getBlocks(validSchema)`.
pub fn get_blocks(valid_schema: &Value) -> Blocks<'_> {
    let mut out = Blocks::default();
    let root_blocks = valid_schema.get("blocks").and_then(Value::as_array);
    out.has_root_blocks_declaration = root_blocks.is_some();
    for (i, block) in root_blocks.unwrap_or(&[]).iter().enumerate() {
        if !block.is_truthy() {
            continue;
        }
        let path = vec!["blocks".to_string(), i.to_string(), "type".to_string()];
        if has(block, "name") {
            out.root_level_local_blocks.push(BlockWithPath { node: block, path });
        } else if block_type(block) != "@app" {
            out.root_level_theme_blocks.push(BlockWithPath { node: block, path });
        }
    }
    if let Some(presets) = valid_schema.get("presets").and_then(Value::as_array) {
        for (pi, preset) in presets.iter().enumerate() {
            match preset.get("blocks") {
                Some(Value::Array(a)) => {
                    for (bi, block) in a.iter().enumerate() {
                        let path = vec!["presets".into(), pi.to_string(), "blocks".into(), bi.to_string()];
                        categorize_preset(&mut out, block, path, 0);
                    }
                }
                Some(Value::Object(o)) => {
                    for (key, block) in o.iter() {
                        let path = vec!["presets".into(), pi.to_string(), "blocks".into(), key.to_string()];
                        categorize_preset(&mut out, block, path, 0);
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(default_blocks) = valid_schema.get("default").and_then(|d| d.get("blocks")).and_then(Value::as_array) {
        for (i, block) in default_blocks.iter().enumerate() {
            if has(block, "name") {
                out.default_level_blocks.push(BlockWithPath {
                    node: block,
                    path: vec!["default".into(), "blocks".into(), i.to_string(), "type".into()],
                });
            }
        }
    }
    out
}

/// `isInvalidPresetBlock(blockId, blockNode, rootLevelThemeBlocks, staticBlockDefs, hasRootBlocksDeclaration = true)`.
pub fn is_invalid_preset_block(
    block_id: &str,
    block: &Value,
    root_theme_blocks: &[BlockWithPath<'_>],
    static_defs: &[StaticBlockDef],
    has_root_declaration: bool,
) -> bool {
    let ty = block_type(block);
    if block.get("static").is_some_and(Value::is_truthy) {
        return !static_defs.iter().any(|d| d.ty == ty && d.id == block_id);
    }
    let private = ty.starts_with('_');
    if private && !has_root_declaration {
        return false;
    }
    let theme_in_root = root_theme_blocks.iter().any(|b| block_type(b.node) == "@theme");
    let needs_explicit = private || !theme_in_root;
    let in_root = root_theme_blocks.iter().any(|b| block_type(b.node) == ty);
    !in_root && needs_explicit
}

/// `isInvalidDefaultBlock(blockNode, rootLevelThemeBlocks, hasRootBlocksDeclaration = true)`.
pub fn is_invalid_default_block(block: &Value, root_theme_blocks: &[BlockWithPath<'_>], has_root_declaration: bool) -> bool {
    let ty = block_type(block);
    let private = ty.starts_with('_');
    if private && !has_root_declaration {
        return false;
    }
    let theme_in_root = root_theme_blocks.iter().any(|b| block_type(b.node) == "@theme");
    let needs_explicit = private || !theme_in_root;
    let in_root = root_theme_blocks.iter().any(|b| block_type(b.node) == ty);
    !in_root && needs_explicit
}

/// `reportWarning(message, offset, astNode, context)`.
pub fn report_warning<C: Corrector>(cx: &Cx<'_, '_, C>, message: String, offset: u32, node: &JsonNode<'_>) {
    let loc = node.loc();
    cx.report(Problem::new(message, offset + loc.start, offset + loc.end));
}

/// `validateBlockFileExistence(blockType, context)`.
pub fn validate_block_file_existence<C>(block_type: &str, cx: &Cx<'_, '_, C>) -> bool {
    block_type == "@theme" || block_type == "@app" || cx.does_file_exist(&format!("blocks/{block_type}.liquid"))
}

/// Relative ticks of upstream's non-I/O awaits when fanning out with `Promise.all`.
const SCHEMA_TICKS: u32 = 3;
const BUILTIN_TICKS: u32 = 1;

/// `await validateBlockFileExistence(blockType, context)`, then `task`.
pub fn after_existence_check<'t>(s: &mut Sched<'t>, block_type: &str, task: impl FnOnce(&mut Sched<'t>) + 't) {
    if block_type == "@theme" || block_type == "@app" {
        s.after(BUILTIN_TICKS, task);
    } else {
        s.after_io(task);
    }
}

/// What `validateNestedBlocks` and `validateBlockTargeting` close over.
#[derive(Clone)]
pub struct NestedBlocks<'t, 'a, 'f> {
    pub cx: &'t LiquidCx<'a, 'f>,
    pub offset: u32,
    pub ast: &'t JsonNode<'a>,
}

impl<'t, 'a: 't, 'f> NestedBlocks<'t, 'a, 'f> {
    /// `validateNestedBlocks(context, parentNode, nestedBlocks, currentPath, offset, ast)`.
    pub fn validate_nested_blocks(&self, s: &mut Sched<'t>, parent: &'t Value, nested: &'t Value, current_path: Vec<String>) {
        if !nested.is_truthy() {
            return;
        }
        let this = self.clone();
        s.after(SCHEMA_TICKS, move |s| {
            let Some(parent_schema) = this.cx.block_schema(block_type(parent)) else { return };
            let Some(valid) = parent_schema.valid_value() else { return };
            let blocks = get_blocks(valid);
            let allowed = std::rc::Rc::new(blocks.root_level_theme_blocks.iter().map(|b| block_type(b.node).to_string()).collect::<Vec<_>>());
            let theme_blocks = std::rc::Rc::new(blocks.root_level_theme_blocks);
            for (key, block) in crate::checks::shared_schema::entries(nested) {
                let mut path = current_path.clone();
                path.extend(["blocks".to_string(), key.into_owned(), "type".to_string()]);
                this.validate_block_targeting(s, block, path, parent, theme_blocks.clone(), allowed.clone(), &parent_schema.static_block_defs);
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_block_targeting(
        &self,
        s: &mut Sched<'t>,
        nested: &'t Value,
        nested_path: Vec<String>,
        parent: &'t Value,
        root_theme_blocks: std::rc::Rc<Vec<BlockWithPath<'a>>>,
        allowed: std::rc::Rc<Vec<String>>,
        static_defs: &[StaticBlockDef],
    ) {
        let ty = block_type(nested);
        let (cx, offset) = (self.cx, self.offset);
        let type_node = crate::checks::shared_schema::node_at(self.ast, &nested_path);
        let nested_blocks = nested.get("blocks").filter(|b| b.is_truthy());
        let this = self.clone();
        let parent_path = nested_path[..nested_path.len() - 1].to_vec();
        let then = move |s: &mut Sched<'t>| {
            if let Some(blocks) = nested_blocks {
                this.validate_nested_blocks(s, nested, blocks, parent_path);
            }
        };
        let Some(type_node) = type_node else {
            then(s);
            return;
        };
        let block_id = match nested.get("id") {
            Some(id) => crate::checks::shared_schema::js_string(Some(id)).into_owned(),
            None => nested_path.get(nested_path.len().wrapping_sub(2)).cloned().unwrap_or_default(),
        };
        if is_invalid_preset_block(&block_id, nested, &root_theme_blocks, static_defs, true) {
            let parent_type = block_type(parent);
            let message = if nested.get("static").is_some_and(Value::is_truthy) {
                format!("Could not find a static block of type \"{ty}\" with id \"{block_id}\" in \"blocks/{parent_type}.liquid\".")
            } else if ty.starts_with('_') {
                format!("Private block type \"{ty}\" is not allowed in \"{parent_type}\" blocks.")
            } else {
                format!(
                    "Block type \"{ty}\" is not allowed in \"{parent_type}\" blocks. Allowed types are: {}.",
                    allowed.join(", ")
                )
            };
            report_warning(cx, message, offset, type_node);
        }
        after_existence_check(s, ty, move |s| {
            if !validate_block_file_existence(ty, cx) {
                report_warning(cx, format!("Theme block 'blocks/{ty}.liquid' does not exist."), offset, type_node);
            }
            then(s);
        });
    }
}
