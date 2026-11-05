//! `ValidSchemaName`.

use super::shared_schema::*;
use crate::api::*;
use lq_json::ast::node_at_path;
use lq_json::{JsonNode, Value};
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

const MAX_SCHEMA_NAME_LENGTH: f64 = 25.0;

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check))
}

struct Check;

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Some(tag) = schema_raw_tag(node) else { return };
        let Some(parts) = valid_parts(cx) else { return };
        if let Err(e) = check_name(parts.valid, parts.ast, tag.block_start_position.end, cx) {
            cx.fail(e);
        }
    }
}

fn check_name(valid: &Value, ast: &JsonNode<'_>, offset: u32, cx: &LiquidCx<'_, '_>) -> Js<()> {
    let Some(name) = valid.get("name").filter(|n| n.is_truthy()) else { return Ok(()) };
    let loc = node_at_path(ast, &["name"])
        .map(JsonNode::loc)
        .ok_or_else(|| type_error("Cannot read properties of undefined (reading 'loc')"))?;
    let (start, end) = (offset + loc.start, offset + loc.end);
    // Anything but a string: `name.startsWith` is not a function.
    let name = name.as_str().ok_or_else(|| type_error("name.startsWith is not a function"))?;
    if let Some(key) = name.strip_prefix("t:") {
        let path: Vec<&str> = key.split('.').collect();
        let translation = deep_get(cx.default_schema_translations(), &path, cx.divergence(Divergence::PrototypeLookup));
        if translation != DeepValue::Undefined && translation.js_length()?.is_some_and(|l| l > MAX_SCHEMA_NAME_LENGTH) {
            cx.report(LiquidProblem::new(
                format!(
                    "Schema name '{}' from 'locales/{}.default.schema.json' is too long (max 25 characters)",
                    translation.display(),
                    cx.default_locale()
                ),
                start,
                end,
            ));
        }
    } else if crate::utils::js_len(name) as f64 > MAX_SCHEMA_NAME_LENGTH {
        cx.report(LiquidProblem::new(format!("Schema name '{name}' is too long (max 25 characters)"), start, end));
    }
    Ok(())
}
