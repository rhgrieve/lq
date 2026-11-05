//! `DeprecatedFontsOnSettingsSchema`.

use super::shared_schema::{find_property, is_deprecated_font, literal_str};
use crate::api::*;
use lq_json::JsonNode;

pub const IMPL: CheckImpl = CheckImpl::Json {
    create,
    enter: JsonMask::of(&[JsonKind::Property]),
    exit: JsonMask::NONE,
};

fn create<'a, 'f>(cx: &JsonCx<'a, 'f>) -> Option<Box<dyn JsonCheck<'a> + 'a>> {
    (cx.to_relative_path(cx.uri()) == "config/settings_schema.json").then(|| Box::new(Check) as Box<dyn JsonCheck<'a> + 'a>)
}

struct Check;

impl<'a> JsonCheck<'a> for Check {
    fn enter(&mut self, node: &'a JsonNode<'a>, _: &[&'a JsonNode<'a>], cx: &JsonCx<'a, '_>) {
        if node.property_key() != Some("settings") {
            return;
        }
        let Some(JsonNode::Array { children, .. }) = node.property_value() else { return };
        for setting in children {
            if !matches!(setting, JsonNode::Object { .. }) {
                continue;
            }
            let is_font_picker = find_property(setting, "type")
                .and_then(JsonNode::property_value)
                .and_then(literal_str)
                == Some("font_picker");
            if !is_font_picker {
                continue;
            }
            let Some(default) = find_property(setting, "default").and_then(JsonNode::property_value) else { continue };
            let Some(font) = literal_str(default) else { continue };
            if is_deprecated_font(font) {
                let loc = default.loc();
                cx.report(JsonProblem::new(format!("The font \"{font}\" is deprecated"), loc.start, loc.end));
            }
        }
    }
}
