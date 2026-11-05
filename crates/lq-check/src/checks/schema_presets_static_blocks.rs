//! `SchemaPresetsStaticBlocks`.

use super::shared_schema::*;
use crate::api::*;
use crate::check_utils::is_content_for_block;
use lq_json::ast::{node_at_path, PathKey};
use lq_json::{JsonNode, Value};
use lq_syntax::{Markup, Node};
use std::borrow::Cow;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag, NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check { content_for_blocks: Vec::new(), static_blocks: Vec::new() }))
}

struct StaticBlock<'a> {
    /// An array block's `id`, or a hash block's key.
    id: Cow<'a, str>,
    id_is_string: bool,
    ty: Option<&'a Value>,
    start: u32,
    end: u32,
}

struct Check<'a> {
    content_for_blocks: Vec<(&'a str, &'a str)>,
    static_blocks: Vec<StaticBlock<'a>>,
}

fn loc_of(ast: &JsonNode<'_>, path: &[PathKey<'_>]) -> Js<lq_json::Loc> {
    node_at_path(ast, path)
        .map(JsonNode::loc)
        .ok_or_else(|| type_error("Cannot read properties of undefined (reading 'loc')"))
}

impl<'a> Check<'a> {
    fn collect(&mut self, valid: &'a Value, ast: &JsonNode<'_>, offset: u32) -> Js<()> {
        use PathKey::{Index, Str};
        let Some(presets) = valid.get("presets").filter(|p| p.is_truthy()) else { return Ok(()) };
        for (index, preset) in array_of(presets)?.iter().enumerate() {
            if !in_op(preset, "blocks")? {
                continue;
            }
            match preset.get("blocks") {
                Some(Value::Array(blocks)) => {
                    for (block_index, block) in blocks.iter().enumerate() {
                        let id = prop(block, "id")?;
                        if block.get("static").and_then(Value::as_bool) == Some(true) {
                            if let Some(id) = id.filter(|i| i.is_truthy()) {
                                let loc = loc_of(ast, &[Str("presets"), Index(index), Str("blocks"), Index(block_index)])?;
                                self.static_blocks.push(StaticBlock {
                                    id: js_string(Some(id)),
                                    id_is_string: id.as_str().is_some(),
                                    ty: block.get("type"),
                                    start: offset + loc.start,
                                    end: offset + loc.end,
                                });
                            }
                        }
                    }
                }
                Some(Value::Object(blocks)) => {
                    for (block_id, block) in blocks.iter() {
                        if prop(block, "static")?.and_then(Value::as_bool) == Some(true) {
                            let loc = loc_of(ast, &[Str("presets"), Index(index), Str("blocks"), Str(block_id)])?;
                            self.static_blocks.push(StaticBlock {
                                id: Cow::Borrowed(block_id),
                                id_is_string: true,
                                ty: block.get("type"),
                                start: offset + loc.start,
                                end: offset + loc.end,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl<'a> LiquidCheck<'a> for Check<'a> {
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        match node {
            Node::LiquidTag(tag) => {
                if tag.name != "content_for" || !is_content_for_block(&tag.markup) {
                    return;
                }
                let Markup::Node(markup) = &tag.markup else { return };
                let Node::ContentForMarkup(markup) = &**markup else { return };
                let arg = |name: &str| {
                    markup.args.iter().find_map(|a| match a {
                        Node::NamedArgument(n) if n.name == name => Some(&n.value),
                        _ => None,
                    })
                };
                let Some(Node::String(ty)) = arg("type") else { return };
                if let Some(Node::String(id)) = arg("id") {
                    self.content_for_blocks.push((id.value, ty.value));
                }
            }
            Node::LiquidRawTag(_) => {
                let Some(tag) = schema_raw_tag(node) else { return };
                let Some(parts) = valid_parts(cx) else { return };
                if let Err(e) = self.collect(parts.valid, parts.ast, tag.block_start_position.end) {
                    cx.fail(e);
                }
            }
            _ => {}
        }
    }

    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {
        for block in &self.static_blocks {
            let ty = block.ty.and_then(Value::as_str);
            let found = block.id_is_string && self.content_for_blocks.iter().any(|(i, t)| *i == block.id && Some(*t) == ty);
            if !found {
                cx.report(LiquidProblem::new(
                    format!("Static block {} is missing a corresponding content_for \"block\" tag.", block.id),
                    block.start,
                    block.end,
                ));
            }
        }
    }
}
