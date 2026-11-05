//! `JSONMissingSection`.

use super::shared_schema::{find_property, literal_str};
use crate::api::*;
use lq_json::ast::node_at_path;
use lq_json::JsonNode;

pub const IMPL: CheckImpl = CheckImpl::Json { create, enter: JsonMask::NONE, exit: JsonMask::NONE };

const SECTION_TYPES_WITH_PLATFORM_DEFAULTS: [&str; 2] = ["apps", "_blocks"];

fn create<'a, 'f>(cx: &JsonCx<'a, 'f>) -> Option<Box<dyn JsonCheck<'a> + 'a>> {
    let relative = cx.to_relative_path(cx.uri());
    (relative.starts_with("templates/") || relative.starts_with("sections/")).then(|| Box::new(Check) as Box<dyn JsonCheck<'a> + 'a>)
}

struct Check;

impl<'a> JsonCheck<'a> for Check {
    fn on_code_path_end(&mut self, cx: &JsonCx<'a, '_>) {
        let Ok(ast) = cx.file().json_ast() else { return };
        let Some(sections @ JsonNode::Object { .. }) = node_at_path(ast, &["sections"]) else { return };
        for property in sections.children() {
            let Some(section @ JsonNode::Object { .. }) = property.property_value() else { continue };
            let Some(type_node) = find_property(section, "type").and_then(JsonNode::property_value) else { continue };
            let Some(section_type) = literal_str(type_node) else { continue };
            if SECTION_TYPES_WITH_PLATFORM_DEFAULTS.contains(&section_type) {
                continue;
            }
            if cx.does_file_exist(&format!("sections/{section_type}.liquid")) {
                continue;
            }
            let loc = type_node.loc();
            cx.report(JsonProblem::new(
                format!("Section type '{section_type}' does not refer to an existing section file"),
                loc.start,
                loc.end,
            ));
        }
    }
}
