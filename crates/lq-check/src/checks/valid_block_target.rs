//! `ValidBlockTarget`.

use super::shared_schema::*;
use crate::api::*;
use crate::utils::block::{self, get_blocks, is_invalid_default_block, is_invalid_preset_block, NestedBlocks};
use lq_json::{JsonNode, Value};
use lq_syntax::Node;
use std::cell::RefCell;
use std::rc::Rc;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check))
}

struct Check;

fn missing_definition_error(ty: &str) -> String {
    if ty.starts_with('_') {
        format!("Theme block type \"{ty}\" is a private block so it must be explicitly allowed in \"blocks\" at the root of this schema.")
    } else {
        format!("Theme block type \"{ty}\" must be allowed in \"blocks\" at the root of this schema.")
    }
}

fn does_not_exist_error(ty: &str) -> String {
    format!("Theme block 'blocks/{ty}.liquid' does not exist.")
}

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Some(tag) = schema_raw_tag(node) else { return };
        let Some(parts) = valid_parts(cx) else { return };
        let offset = tag.block_start_position.end;
        let ast = parts.ast;
        let found = get_blocks(parts.valid);
        if !found.root_level_local_blocks.is_empty() {
            return;
        }

        // A missing type node (duplicate keys) throws in a `Promise.all`: the rest of the
        // handler is skipped, while tasks already started run on.
        let failed: Rc<RefCell<Option<lq_json::validator::JsError>>> = Rc::default();
        let reporter = Reporter { cx, offset, failed: failed.clone() };

        let mut errors_in_root = false;
        for b in &found.root_level_theme_blocks {
            let ty = js_string(b.node.get("type"));
            if !block::validate_block_file_existence(&ty, cx) {
                errors_in_root = true;
                reporter.report(does_not_exist_error(&ty), node_at(ast, &b.path));
            }
        }
        if errors_in_root {
            return finish(&failed, cx);
        }

        if let Some(blocks) = found.preset_level_blocks.get(&0) {
            let mut sched = Sched::new();
            let nested_cx = NestedBlocks { cx, offset, ast };
            for b in blocks {
                let ty = js_string(b.node.get("type"));
                let node = node_at(ast, &b.path);
                let block_id = preset_block_id(b.node, &b.path);
                if is_invalid_preset_block(&block_id, b.node, &found.root_level_theme_blocks, &parts.schema.static_block_defs, found.has_root_blocks_declaration) {
                    let message = if b.node.get("static").is_some_and(Value::is_truthy) {
                        format!("Could not find a static block of type \"{ty}\" with id \"{block_id}\" in this file.")
                    } else {
                        missing_definition_error(&ty)
                    };
                    reporter.report(message, node);
                }
                let nested_cx = nested_cx.clone();
                let (bnode, path) = (b.node, b.path.clone());
                let reporter = reporter.clone();
                block::after_existence_check(&mut sched, &ty, move |s| {
                    let ty = js_string(bnode.get("type"));
                    if block::validate_block_file_existence(&ty, nested_cx.cx) {
                        if let Some(nested) = bnode.get("blocks").filter(|n| n.is_truthy()) {
                            nested_cx.validate_nested_blocks(s, bnode, nested, path[..path.len() - 1].to_vec());
                        }
                    } else {
                        reporter.report(does_not_exist_error(&ty), node);
                    }
                });
            }
            sched.run();
        }
        if failed.borrow().is_some() {
            return finish(&failed, cx);
        }

        let mut sched = Sched::new();
        for b in &found.default_level_blocks {
            let ty = js_string(b.node.get("type"));
            let node = node_at(ast, &b.path);
            if is_invalid_default_block(b.node, &found.root_level_theme_blocks, found.has_root_blocks_declaration) {
                reporter.report(missing_definition_error(&ty), node);
            }
            let reporter = reporter.clone();
            let bnode = b.node;
            block::after_existence_check(&mut sched, &ty, move |_| {
                let ty = js_string(bnode.get("type"));
                if !block::validate_block_file_existence(&ty, cx) {
                    reporter.report(does_not_exist_error(&ty), node);
                }
            });
        }
        sched.run();
        finish(&failed, cx);
    }
}

/// `'id' in node ? node.id! : path.at(-2)!`.
fn preset_block_id<'v>(block: &'v Value, path: &'v [String]) -> std::borrow::Cow<'v, str> {
    match block.get("id") {
        Some(id) => js_string(Some(id)),
        None => std::borrow::Cow::Borrowed(path.get(path.len().wrapping_sub(2)).map_or("undefined", String::as_str)),
    }
}

#[derive(Clone)]
struct Reporter<'t, 'a, 'f> {
    cx: &'t LiquidCx<'a, 'f>,
    offset: u32,
    failed: Rc<RefCell<Option<lq_json::validator::JsError>>>,
}

impl Reporter<'_, '_, '_> {
    fn report(&self, message: String, node: Option<&JsonNode<'_>>) {
        match node {
            Some(n) => block::report_warning(self.cx, message, self.offset, n),
            None => {
                self.failed.borrow_mut().get_or_insert_with(|| type_error("Cannot read properties of undefined (reading 'loc')"));
            }
        }
    }
}

fn finish(failed: &Rc<RefCell<Option<lq_json::validator::JsError>>>, cx: &LiquidCx<'_, '_>) {
    if let Some(e) = failed.borrow_mut().take() {
        cx.fail(e);
    }
}
