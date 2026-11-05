//! `ValidBlockContentSettingType`.

use super::shared_args::is_content_parameter;
use crate::api::*;
use crate::schema_settings::schema_setting_liquid_type;
use crate::to_schema::is_block;
use crate::utils::block::report_warning;
use lq_json::ast::{node_at_path, PathKey};
use lq_json::validator::JsError;
use lq_json::Value;
use lq_syntax::{Node, RawMarkupKind};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    if !is_block(cx.uri()) {
        return None;
    }
    Some(Box::new(Check))
}

struct Check;

fn type_error(message: &str) -> JsError {
    JsError { name: "TypeError".into(), message: message.into() }
}

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidRawTag(tag) = node else { return };
        if tag.name != "schema" || !matches!(&tag.body, Node::RawMarkup(b) if b.kind == RawMarkupKind::Json) {
            return;
        }
        let Some(schema) = cx.block_schema(&lq_json::uri::basename(cx.uri(), Some(".liquid"))) else { return };
        let Some(Ok(valid)) = schema.valid_schema() else { return };
        let Ok(ast) = &schema.ast else { return };
        // `undefined.settings` and `null.settings` throw.
        let Some(valid) = valid.as_ref().filter(|v| !matches!(v, Value::Null)) else {
            return cx.fail(type_error("Cannot read properties of undefined (reading 'settings')"));
        };
        let settings = match valid.get("settings") {
            None | Some(Value::Null) => return,
            Some(Value::Array(settings)) => settings,
            Some(_) => return cx.fail(type_error("settings.findIndex is not a function")),
        };
        let mut found = None;
        for (index, setting) in settings.iter().enumerate() {
            if matches!(setting, Value::Null) {
                return cx.fail(type_error("Cannot read properties of null (reading 'id')"));
            }
            if setting.get("id").and_then(Value::as_str).is_some_and(is_content_parameter) {
                found = Some((index, setting));
                break;
            }
        }
        let Some((index, setting)) = found else { return };
        let Some(setting_type) = setting.get("type").and_then(Value::as_str).and_then(schema_setting_liquid_type) else {
            return;
        };
        let path = [PathKey::Str("settings"), PathKey::Index(index), PathKey::Str("type")];
        let Some(type_node) = node_at_path(ast, &path) else { return };
        if setting_type == "string" {
            return;
        }
        report_warning(
            cx,
            format!("Schema setting 'content' has Liquid type '{setting_type}', but the built-in 'content' parameter has type 'string'."),
            schema.offset,
            type_node,
        );
    }
}
