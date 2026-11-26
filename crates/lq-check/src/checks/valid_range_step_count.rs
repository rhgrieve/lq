//! `ValidRangeStepCount`, on Liquid files: a range setting may have at most 101 steps, or the
//! storefront rejects it with "Range settings must have at most 101 steps".

use super::shared_range::*;
use super::shared_schema::*;
use crate::api::*;
use crate::utils::block::report_warning;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

pub(super) const MAX_STEPS: f64 = 101.0;

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    let in_scope = crate::to_schema::is_block(cx.uri()) || crate::to_schema::is_section(cx.uri());
    in_scope.then(|| Box::new(Check) as Box<dyn LiquidCheck<'a> + 'a>)
}

struct Check;

fn raw_step_count(r: &RangeSetting<'_>) -> f64 {
    (r.max - r.min) / r.step + 1.0
}

/// `exceedsMaxSteps(range)`. EPSILON keeps float jitter, like step 0.1 across (1, 5) giving
/// 41.00000000000001, from tripping the comparison.
pub(super) fn exceeds_max_steps(r: &RangeSetting<'_>) -> bool {
    raw_step_count(r) > MAX_STEPS + EPSILON
}

/// `tooManyStepsMessage(range)`.
pub(super) fn too_many_steps_message(r: &RangeSetting<'_>) -> String {
    // `Math.round` rounds halves up.
    let count = (raw_step_count(r) + 0.5).floor();
    format!(
        "Range setting \"{}\" has {} steps (min {}, max {}, step {}). Shopify allows at most {} steps.",
        r.id,
        js(count),
        js(r.min),
        js(r.max),
        js(r.step),
        js(MAX_STEPS)
    )
}

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Some(tag) = schema_raw_tag(node) else { return };
        let Some(parts) = valid_parts(cx) else { return };
        let offset = tag.block_start_position.end;
        for (path, settings) in setting_definitions(parts.valid) {
            for (i, setting) in settings.iter().enumerate() {
                let Some(range) = to_range_setting(setting) else { continue };
                if !exceeds_max_steps(&range) {
                    continue;
                }
                // Without a step, the range is too wide for the default step of 1.
                let at = |key: &str| {
                    let mut p = path.clone();
                    p.extend([i.to_string(), key.to_string()]);
                    node_at(parts.ast, &p)
                };
                let Some(ast_node) = at("step").or_else(|| at("max")) else { continue };
                report_warning(cx, too_many_steps_message(&range), offset, ast_node);
            }
        }
    }
}
