//! `DeprecatedFontsOnSettingsData`.

use super::shared_schema::{is_deprecated_font, literal_str};
use crate::api::*;
use lq_json::JsonNode;

pub const IMPL: CheckImpl = CheckImpl::Json {
    create,
    enter: JsonMask::of(&[JsonKind::Property]),
    exit: JsonMask::NONE,
};

fn create<'a, 'f>(cx: &JsonCx<'a, 'f>) -> Option<Box<dyn JsonCheck<'a> + 'a>> {
    (cx.to_relative_path(cx.uri()) == "config/settings_data.json").then(|| Box::new(Check) as Box<dyn JsonCheck<'a> + 'a>)
}

struct Check;

impl<'a> JsonCheck<'a> for Check {
    fn enter(&mut self, node: &'a JsonNode<'a>, _: &[&'a JsonNode<'a>], cx: &JsonCx<'a, '_>) {
        let JsonNode::Property { value, .. } = node else { return };
        let Some(font) = literal_str(value) else { return };
        if is_deprecated_font(font) {
            let loc = value.loc();
            cx.report(JsonProblem::new(format!("The font \"{font}\" is deprecated"), loc.start, loc.end));
        }
    }
}
