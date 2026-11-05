//! `ExcessiveSettingsCount`.

use super::shared_schema::find_property;
use crate::api::*;
use lq_json::{to_json_node, JsonNode};
use lq_syntax::{LiquidRawTag, Node};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check { schema_node: None }))
}

struct Check<'a> {
    /// `visit(ast, { LiquidRawTag })[0]`: the walk's order is `visit`'s.
    schema_node: Option<&'a LiquidRawTag<'a>>,
}

impl<'a> LiquidCheck<'a> for Check<'a> {
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], _cx: &LiquidCx<'a, '_>) {
        if let Node::LiquidRawTag(tag) = node {
            if tag.name == "schema" && self.schema_node.is_none() {
                self.schema_node = Some(tag);
            }
        }
    }

    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {
        let Some(tag) = self.schema_node else { return };
        let Node::RawMarkup(body) = &tag.body else { return };
        let Ok(ast) = to_json_node(body.value) else { return };
        if !matches!(ast, JsonNode::Object { .. }) {
            return;
        }
        let Some(JsonNode::Array { children, .. }) = find_property(&ast, "settings").and_then(JsonNode::property_value) else {
            return;
        };
        let count = children
            .iter()
            .filter(|s| matches!(s, JsonNode::Object { .. }) && find_property(s, "id").is_some())
            .count();
        let max = cx.settings().f64("maxSettings").unwrap_or(40.0);
        if count as f64 <= max {
            return;
        }
        let max = lq_json::js::number_to_string(max);
        cx.report(LiquidProblem::new(
            format!("This schema declares {count} settings, which exceeds the maximum of {max}. Consider splitting this section or block into smaller pieces, or grouping related options with a header."),
            tag.block_start_position.start,
            tag.block_start_position.end,
        ));
    }
}
