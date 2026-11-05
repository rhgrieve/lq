//! `UniqueSettingIds`, whose code is `UniqueSettingId`.

use super::shared_schema::{find_property, literal_str};
use crate::api::*;
use lq_json::JsonNode;
use std::collections::HashMap;

pub const IMPL: CheckImpl = CheckImpl::Json { create, enter: JsonMask::NONE, exit: JsonMask::NONE };

fn create<'a, 'f>(_cx: &JsonCx<'a, 'f>) -> Option<Box<dyn JsonCheck<'a> + 'a>> {
    Some(Box::new(Check))
}

struct Check;

impl<'a> JsonCheck<'a> for Check {
    fn on_code_path_end(&mut self, cx: &JsonCx<'a, '_>) {
        let Ok(ast @ JsonNode::Array { .. }) = cx.file().json_ast() else { return };
        // `Map<id, nodes>`: first-seen order, last node.
        let mut ids: Vec<(&str, &JsonNode<'_>, u32)> = Vec::new();
        let mut index: HashMap<&str, usize> = HashMap::new();
        for group in ast.children() {
            if !matches!(group, JsonNode::Object { .. }) {
                continue;
            }
            let Some(JsonNode::Array { children, .. }) = find_property(group, "settings").and_then(JsonNode::property_value) else {
                continue;
            };
            for setting in children {
                if !matches!(setting, JsonNode::Object { .. }) {
                    continue;
                }
                let Some(id_node) = find_property(setting, "id") else { continue };
                let Some(id) = id_node.property_value().and_then(literal_str) else { continue };
                match index.get(id) {
                    Some(&i) => {
                        ids[i].1 = id_node;
                        ids[i].2 += 1;
                    }
                    None => {
                        index.insert(id, ids.len());
                        ids.push((id, id_node, 1));
                    }
                }
            }
        }
        for (id, node, count) in ids {
            if count > 1 {
                let loc = node.loc();
                cx.report(JsonProblem::new(format!("Duplicate setting id found: \"{id}\""), loc.start, loc.end));
            }
        }
    }
}
