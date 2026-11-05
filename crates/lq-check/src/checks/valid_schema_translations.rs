//! `ValidSchemaTranslations`.

use super::shared_schema::{deep_get, schema_raw_tag, DeepValue};
use crate::api::*;
use lq_json::{JsonNode, Literal};
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
        let Some(schema) = cx.schema() else { return };
        // Upstream's `getSchema` rejects when validation rejects.
        if let Some(Err(e)) = schema.shared_validation() {
            cx.fail(e.clone());
            return;
        }
        let Ok(ast) = &schema.ast else { return };
        find_translation_keys(ast, tag.block_start_position.end, cx);
    }
}

/// `findTranslationKeys(ast)`, checking each as it is found.
fn find_translation_keys(node: &JsonNode<'_>, offset: u32, cx: &LiquidCx<'_, '_>) {
    match node {
        JsonNode::Literal { value: Literal::String(value), loc } => {
            let Some(key) = value.strip_prefix("t:") else { return };
            let path: Vec<&str> = key.split('.').collect();
            if deep_get(cx.default_schema_translations(), &path, cx.divergence(Divergence::PrototypeLookup)) == DeepValue::Undefined {
                cx.report(LiquidProblem::new(
                    format!("'{value}' does not have a matching entry in 'locales/{}.default.schema.json'", cx.default_locale()),
                    offset + loc.start,
                    offset + loc.end,
                ));
            }
        }
        JsonNode::Object { children, .. } => {
            for property in children {
                if let Some(value) = property.property_value() {
                    find_translation_keys(value, offset, cx);
                }
            }
        }
        JsonNode::Array { children, .. } => {
            for child in children {
                find_translation_keys(child, offset, cx);
            }
        }
        JsonNode::Property { .. } | JsonNode::Identifier { .. } | JsonNode::Literal { .. } => {}
    }
}
