//! `ValidSelectDefault`, on `config/settings_schema.json`.

use super::shared_select::check_defaults;
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
        check_defaults(node.property_value(), &mut |message, n| {
            let loc = n.loc();
            cx.report(JsonProblem::new(message, loc.start, loc.end));
        });
    }
}
