//! `ValidRangeStepCount`, on `config/settings_schema.json`.

use super::shared_range::*;
use super::valid_range_step_count::{exceeds_max_steps, too_many_steps_message};
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
            let JsonNode::Object { children: props, .. } = setting else { continue };
            let values = literal_values(props);
            let Some(range) = to_range_setting(&values) else { continue };
            if !exceeds_max_steps(&range) {
                continue;
            }
            let Some(ast_node) = property(props, "step").or_else(|| property(props, "max")) else { continue };
            let loc = ast_node.loc();
            cx.report(JsonProblem::new(too_many_steps_message(&range), loc.start, loc.end));
        }
    }
}
