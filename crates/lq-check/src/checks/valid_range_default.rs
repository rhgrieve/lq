//! `ValidRangeDefault`, on Liquid files: a range setting's default, or the value a preset or a
//! section's `default` gives it, must be in [min, max] and on the step grid. The storefront
//! rejects such a schema at render time.

use super::shared_range::*;
use super::shared_schema::*;
use crate::api::*;
use crate::to_schema::ThemeSchemaType;
use crate::utils::block::report_warning;
use lq_json::{JsonNode, Literal};
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    // `getSchema` is undefined elsewhere, and nothing else can report.
    let in_scope = crate::to_schema::is_block(cx.uri()) || crate::to_schema::is_section(cx.uri());
    in_scope.then(|| Box::new(Check) as Box<dyn LiquidCheck<'a> + 'a>)
}

struct Check;

/// `rangeProblem(range, value)`.
pub(super) fn range_problem(range: &RangeSetting<'_>, value: f64) -> Option<String> {
    let (min, max) = (js(range.min), js(range.max));
    if value < range.min || value > range.max {
        return Some(format!("is outside the range [{min}, {max}]"));
    }
    let ratio = (value - range.min) / range.step;
    // `Math.round` rounds halves up.
    ((ratio - (ratio + 0.5).floor()).abs() >= EPSILON)
        .then(|| format!("is not aligned to step {} in range [{min}, {max}]", js(range.step)))
}

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Some(tag) = schema_raw_tag(node) else { return };
        let Some(parts) = valid_parts(cx) else { return };
        let offset = tag.block_start_position.end;
        let report = |n: Option<&JsonNode<'_>>, message: String| {
            if let Some(n) = n {
                report_warning(cx, message, offset, n);
            }
        };

        // 1. The defaults of the schema's settings and of its local blocks' settings.
        for (path, settings) in setting_definitions(parts.valid) {
            for (i, setting) in settings.iter().enumerate() {
                let Some(range) = to_range_setting(setting) else { continue };
                let Some(default) = finite(range.default) else { continue };
                let Some(problem) = range_problem(&range, default) else { continue };
                let mut at = path.clone();
                at.extend([i.to_string(), "default".to_string()]);
                report(
                    node_at(parts.ast, &at),
                    format!("Default value {} for range setting \"{}\" {problem}.", js(default), range.id),
                );
            }
        }

        // 2. The values that presets and a section's default give the schema's settings. A `Map`:
        // a later setting with the same id wins.
        let mut ranges: Vec<RangeSetting<'_>> = Vec::new();
        for setting in parts.valid.get("settings").and_then(|s| s.as_array()).unwrap_or(&[]) {
            if let Some(range) = to_range_setting(setting) {
                ranges.retain(|r| r.id != range.id);
                ranges.push(range);
            }
        }
        if ranges.is_empty() {
            return;
        }
        let mut value_paths: Vec<Vec<String>> = Vec::new();
        if let Some(presets) = parts.valid.get("presets").and_then(|p| p.as_array()) {
            for i in 0..presets.len() {
                value_paths.push(vec!["presets".into(), i.to_string(), "settings".into()]);
            }
        }
        if parts.schema.ty == ThemeSchemaType::Section {
            value_paths.push(vec!["default".into(), "settings".into()]);
        }
        for path in value_paths {
            let Some(JsonNode::Object { children, .. }) = node_at(parts.ast, &path) else { continue };
            for property in children {
                let Some(range) = property.property_key().and_then(|k| ranges.iter().find(|r| r.id == k)) else { continue };
                let Some(value_node) = property.property_value() else { continue };
                let JsonNode::Literal { value: Literal::Number(value), .. } = value_node else { continue };
                if !value.is_finite() {
                    continue;
                }
                let Some(problem) = range_problem(range, *value) else { continue };
                report(Some(value_node), format!("Value {} for range setting \"{}\" {problem}.", js(*value), range.id));
            }
        }
    }
}
