//! `JSONMissingBlock` (with `missing-block-utils.ts`).

use super::shared_schema::*;
use crate::api::*;
use crate::to_schema::parsed_json;
use lq_json::{JsonNode, Value};

pub const IMPL: CheckImpl = CheckImpl::Json { create, enter: JsonMask::NONE, exit: JsonMask::NONE };

/// Relative ticks of upstream's non-I/O awaits. Blocks fan out with `Promise.all`, so these
/// decide the report order within a file.
const SCHEMA_TICKS: u32 = 3;
const RETURN_TICKS: u32 = 1;

fn create<'a, 'f>(cx: &JsonCx<'a, 'f>) -> Option<Box<dyn JsonCheck<'a> + 'a>> {
    cx.to_relative_path(cx.uri()).starts_with("templates/").then(|| Box::new(Check) as Box<dyn JsonCheck<'a> + 'a>)
}

struct Check;

/// `isPropertyNode(x)`: an object or array.
fn is_container(v: &Value) -> bool {
    matches!(v, Value::Object(_) | Value::Array(_))
}

impl<'a> JsonCheck<'a> for Check {
    fn on_code_path_end(&mut self, cx: &JsonCx<'a, '_>) {
        let Ok(ast) = cx.file().json_ast() else { return };
        let Some(parsed) = parsed_json(cx.source()) else { return };
        let Some(sections) = parsed.get("sections").filter(|s| s.is_truthy()) else { return };
        let walk = Walk { cx, ast };
        let mut sched = Sched::new();
        for (key, section) in entries(sections) {
            if !is_container(section) {
                continue;
            }
            let Some(blocks) = section.get("blocks").filter(|b| is_container(b)) else { continue };
            if !matches!(in_op(section, "type"), Ok(true)) {
                continue;
            }
            let path = vec!["sections".to_string(), key.into_owned(), "blocks".to_string()];
            walk.get_all_blocks(&mut sched, section.get("type"), blocks, path);
        }
        sched.run();
    }
}

#[derive(Clone, Copy)]
struct Walk<'t, 'a, 'f> {
    cx: &'t JsonCx<'a, 'f>,
    ast: &'t JsonNode<'a>,
}

/// `path` is `["sections", key, "blocks", (key, "blocks")*]`. Upstream counts every `blocks`
/// segment, so a section or block keyed `blocks` looks nested; with `structural`
/// ([`Divergence::NestedBlockPath`]) keys are skipped.
fn is_nested_block(path: &[String], structural: bool) -> bool {
    if structural {
        path.iter().skip(2).step_by(2).filter(|s| *s == "blocks").count() > 1
    } else {
        path.iter().filter(|s| *s == "blocks").count() > 1
    }
}

impl<'t, 'a: 't, 'f> Walk<'t, 'a, 'f> {
    /// `getAllowedBlocks(sectionType, currentPath, context)`: the host's `blocks`, or `None` when
    /// its schema is missing or invalid, which other checks report (theme-tools 0dcea3cd).
    fn allowed_blocks(&self, section_type: Option<&'t Value>, path: &[String]) -> Option<&'t [Value]> {
        let name = section_type.and_then(Value::as_str).filter(|n| !n.is_empty())?;
        let schema = if is_nested_block(path, self.cx.divergence(Divergence::NestedBlockPath)) { self.cx.block_schema(name) } else { self.cx.section_schema(name) };
        let valid = schema?.valid_value()?;
        Some(match valid.get("blocks") {
            Some(Value::Array(blocks)) => blocks,
            _ => &[],
        })
    }

    /// The part of `validateBlock` before the file lookup: `Ok(Some(themeBlocks))` when it goes
    /// on to look the block's file up, `Ok(None)` when it's done. `Err` is a throw (`in` on a
    /// primitive in `blocks`).
    fn pre_validate(&self, block_type: &str, node: &'t JsonNode<'a>, ancestor: Option<&'t Value>, path: &[String], allowed: &'t [Value]) -> Js<Option<Vec<Option<&'t Value>>>> {
        let location = if is_nested_block(path, self.cx.divergence(Divergence::NestedBlockPath)) { "blocks" } else { "sections" };
        let ty_is = |block: &Value, t: &str| block.get("type").and_then(Value::as_str) == Some(t);
        // App blocks are remote references and need permission from their immediate host.
        if block_type.starts_with("shopify://apps/") {
            let mut app_allowed = false;
            for block in allowed {
                if !in_op(block, "name")? && ty_is(block, "@app") {
                    app_allowed = true;
                    break;
                }
            }
            if !app_allowed {
                let loc = node.loc();
                let ancestor = js_string(ancestor);
                self.cx.report(JsonProblem::new(
                    format!("Block type '{block_type}' is not allowed in '{location}/{ancestor}.liquid'."),
                    loc.start,
                    loc.end,
                ));
            }
            return Ok(None);
        }
        // Section blocks don't need a file in blocks/.
        for block in allowed {
            if in_op(block, "name")? && ty_is(block, block_type) {
                return Ok(None);
            }
        }
        let mut themes = Vec::new();
        for block in allowed {
            if !in_op(block, "name")? && !ty_is(block, "@app") {
                themes.push(block.get("type"));
            }
        }
        Ok((!themes.is_empty()).then_some(themes))
    }

    fn validate_block_result(&self, ty: &'t Value, is_static: bool, node: &'t JsonNode<'a>, ancestor: Option<&'t Value>, path: &[String], themes: &[Option<&'t Value>]) -> Js<()> {
        let name = js_string(Some(ty));
        let loc = node.loc();
        let exists = name == "@theme" || name == "@app" || self.cx.does_file_exist(&format!("blocks/{name}.liquid"));
        if !exists {
            self.cx.report(JsonProblem::new(format!("Theme block 'blocks/{name}.liquid' does not exist."), loc.start, loc.end));
            return Ok(());
        }
        if is_static {
            return Ok(());
        }
        let block_type = ty.as_str().ok_or_else(|| type_error("blockType.startsWith is not a function"))?;
        let includes_at_theme = themes.iter().any(|t| t.and_then(|v| v.as_str()) == Some("@theme"));
        let includes_type = themes.iter().any(|t| t.and_then(|v| v.as_str()) == Some(block_type));
        let allowed = if block_type.starts_with('_') { includes_type } else { includes_type || includes_at_theme };
        if !allowed {
            let location = if is_nested_block(path, self.cx.divergence(Divergence::NestedBlockPath)) { "blocks" } else { "sections" };
            let ancestor = js_string(ancestor);
            self.cx.report(JsonProblem::new(
                format!("Block type '{block_type}' is not allowed in '{location}/{ancestor}.liquid'."),
                loc.start,
                loc.end,
            ));
        }
        Ok(())
    }

    /// `getAllBlocks(ast, 0, ancestorType, blocks, currentPath, context)`. Each block validates
    /// itself, then its nested blocks; siblings run concurrently.
    fn get_all_blocks(self, s: &mut Sched<'t>, ancestor: Option<&'t Value>, blocks: &'t Value, path: Vec<String>) {
        for (key, block) in entries(blocks) {
            // `block.type` throws on `null`, rejecting only this block's promise.
            let Ok(ty) = prop(block, "type") else { continue };
            let mut type_path = path.clone();
            type_path.extend([key.to_string(), "type".to_string()]);
            let mut nested_path = path.clone();
            nested_path.extend([key.to_string(), "blocks".to_string()]);
            let node = node_at(self.ast, &type_path);
            let then = move |s: &mut Sched<'t>| {
                // `'blocks' in block` throws on primitives.
                if matches!(in_op(block, "blocks"), Ok(true)) {
                    if let Some(nested) = block.get("blocks") {
                        self.get_all_blocks(s, block.get("type"), nested, nested_path);
                    }
                }
            };
            // `typeof block.type === 'string' && block.type`
            match (ty.filter(|t| t.as_str().is_some_and(|t| !t.is_empty())), node) {
                (Some(ty), Some(node)) => {
                    let current = path.clone();
                    s.after(SCHEMA_TICKS, move |s| {
                        let Some(allowed) = self.allowed_blocks(ancestor, &current) else {
                            s.after(RETURN_TICKS, then);
                            return;
                        };
                        let themes = match self.pre_validate(ty.as_str().unwrap_or(""), node, ancestor, &current, allowed) {
                            Err(_) => return,
                            Ok(None) => {
                                s.after(RETURN_TICKS, then);
                                return;
                            }
                            Ok(Some(themes)) => themes,
                        };
                        let finish = move |s: &mut Sched<'t>| {
                            let is_static = block.get("static").is_some_and(Value::is_truthy);
                            if self.validate_block_result(ty, is_static, node, ancestor, &current, &themes).is_ok() {
                                s.after(RETURN_TICKS, then);
                            }
                        };
                        let name = js_string(Some(ty));
                        if name == "@theme" || name == "@app" {
                            s.after(RETURN_TICKS, finish);
                        } else {
                            s.after_io(finish);
                        }
                    });
                }
                _ => then(s),
            }
        }
    }
}
