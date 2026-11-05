//! `LiquidFreeSettings`.

use super::shared_schema::{literal_str, schema_raw_tag};
use crate::api::*;
use lq_json::{to_json_node, JsonNode};
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check))
}

struct Check;

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Some(tag) = schema_raw_tag(node) else { return };
        let offset = tag.block_start_position.end;
        let json = &cx.source()[offset as usize..tag.block_end_position.start as usize];
        // A decoded value can only hold `{%` if the text does, or spells it with escapes.
        if !json.contains('%') && !json.contains("\\u") {
            return;
        }
        let Ok(root) = to_json_node(json) else { return };
        let mut ancestors = Vec::with_capacity(16);
        walk(&root, &mut ancestors, offset, cx);
    }
}

/// `visit(jsonFile, { Property })`, with lineage.
fn walk<'j>(node: &'j JsonNode<'j>, ancestors: &mut Vec<&'j JsonNode<'j>>, offset: u32, cx: &LiquidCx<'_, '_>) {
    if let JsonNode::Property { value, .. } = node {
        if let Some(text) = literal_str(value) {
            if node.property_key() != Some("visible_if")
                && text.contains("{%")
                && text.contains("%}")
                && is_in_array_with_parent_key(ancestors, "settings")
                && !is_liquid_type(ancestors)
            {
                let loc = value.loc();
                cx.report(LiquidProblem::new(
                    "Settings values cannot contain liquid logic.",
                    offset + loc.start,
                    offset + loc.end,
                ));
            }
        }
    }
    ancestors.push(node);
    crate::visitor::for_each_json_child(node, |child| walk(child, ancestors, offset, cx));
    ancestors.pop();
}

fn is_liquid_type(ancestors: &[&JsonNode<'_>]) -> bool {
    let Some(parent @ JsonNode::Object { .. }) = ancestors.last() else { return false };
    parent.children().iter().any(|p| {
        p.property_key() == Some("type") && p.property_value().and_then(literal_str) == Some("liquid")
    })
}

fn is_in_array_with_parent_key(ancestors: &[&JsonNode<'_>], key: &str) -> bool {
    ancestors.windows(2).any(|w| {
        matches!(w[1], JsonNode::Array { .. } | JsonNode::Object { .. }) && w[0].property_key() == Some(key)
    })
}
