//! `ValidVisibleIf`, on `config/settings_schema.json`.

use super::shared_schema::{advance_utf16, js_string, literal_str};
use crate::api::*;
use crate::visible_if::{match_index_units, global_settings, lookup_error, settings_scope, validate_lookup, variable_lookups_in_expression, ExpressionLookups, OFFSET_ADJUST};
use lq_json::{JsonNode, Object, Value};

pub const IMPL: CheckImpl = CheckImpl::Json {
    create,
    enter: JsonMask::of(&[JsonKind::Property]),
    exit: JsonMask::NONE,
};

fn create<'a, 'f>(cx: &JsonCx<'a, 'f>) -> Option<Box<dyn JsonCheck<'a> + 'a>> {
    (cx.to_relative_path(cx.uri()) == "config/settings_schema.json")
        .then(|| Box::new(Check { vars: None }) as Box<dyn JsonCheck<'a> + 'a>)
}

struct Check {
    vars: Option<Value>,
}

impl<'a> JsonCheck<'a> for Check {
    fn enter(&mut self, node: &'a JsonNode<'a>, _: &[&'a JsonNode<'a>], cx: &JsonCx<'a, '_>) {
        if node.property_key() != Some("visible_if") {
            return;
        }
        let Some(value) = node.property_value() else { return };
        let Some(expression) = literal_str(value) else { return };
        let loc = value.loc();
        let lookups = match variable_lookups_in_expression(expression) {
            ExpressionLookups::None => return,
            ExpressionLookups::Warning(warning) => {
                cx.report(JsonProblem::new(warning, loc.start, loc.end));
                return;
            }
            ExpressionLookups::Lookups(lookups) => lookups,
        };
        let vars = self.vars.get_or_insert_with(|| {
            let mut vars = Object::new();
            vars.insert("settings", settings_scope(global_settings(cx).iter().map(|s| js_string(Some(s)))));
            Value::Object(vars)
        });
        for lookup in &lookups {
            let message = match lookup.name.as_deref() {
                // settings_schema.json can't reference `section` or `block`.
                Some("section") => Some("Invalid visible_if: can't refer to \"section\" when not in a section file.".to_string()),
                Some("block") => Some("Invalid visible_if: can't refer to \"block\" when not in a block file.".to_string()),
                _ => match validate_lookup(lookup, vars, cx.divergence(Divergence::PrototypeLookup)) {
                    Ok(message) => message,
                    Err(e) => {
                        cx.fail(lookup_error(&e));
                        return;
                    }
                },
            };
            if let Some(message) = message {
                let index = if cx.divergence(Divergence::VisibleIfMatchIndex) { match_index_units(expression) } else { 0 };
                let adjust = |units: u32| advance_utf16(cx.source(), loc.start, (units as i64 + i64::from(index) + OFFSET_ADJUST + 1) as u32);
                cx.report(JsonProblem::new(message, adjust(lookup.start_units), adjust(lookup.end_units)));
            }
        }
    }
}
