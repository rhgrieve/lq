//! `valid-range-default/range-setting.ts`, shared by `ValidRangeDefault` and
//! `ValidRangeStepCount`.

use lq_json::js::number_to_string;
use lq_json::{JsonNode, Literal, Object, Value};

/// Absorbs float error, so `step: 0.1, default: 4.7` counts as on the grid.
pub const EPSILON: f64 = 1e-9;

/// `RangeSetting`: a `range` setting with numeric bounds.
pub struct RangeSetting<'v> {
    pub id: &'v str,
    pub min: f64,
    pub max: f64,
    /// 1 when the setting has no `step`, as on the storefront.
    pub step: f64,
    pub default: Option<&'v Value>,
}

/// `isFiniteNumber(value)`.
pub fn finite(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) if n.is_finite() => Some(*n),
        _ => None,
    }
}

/// `toRangeSetting(setting)`: `None` when it isn't a range setting or its id, min, max or step
/// is invalid. Those are left to ValidSchema. Like the storefront, `max` must exceed `min`.
pub fn to_range_setting(setting: &Value) -> Option<RangeSetting<'_>> {
    if !matches!(setting, Value::Object(_)) || setting.get("type").and_then(Value::as_str) != Some("range") {
        return None;
    }
    let id = setting.get("id").and_then(Value::as_str)?;
    let (min, max) = (finite(setting.get("min"))?, finite(setting.get("max"))?);
    // `step = 1` applies only when `step` is undefined.
    let step = match setting.get("step") {
        None => 1.0,
        some => finite(some)?,
    };
    if step <= 0.0 || max <= min {
        return None;
    }
    Some(RangeSetting { id, min, max, step, default: setting.get("default") })
}

/// `settingDefinitions(schema)`: the schema's own settings, then each local block's, with the
/// path of each list.
pub fn setting_definitions(schema: &Value) -> Vec<(Vec<String>, &[Value])> {
    let mut out = Vec::new();
    if let Some(Value::Array(settings)) = schema.get("settings") {
        out.push((vec!["settings".to_string()], settings.as_slice()));
    }
    if let Some(Value::Array(blocks)) = schema.get("blocks") {
        for (i, block) in blocks.iter().enumerate() {
            if let Some(Value::Array(settings)) = block.get("settings") {
                out.push((vec!["blocks".to_string(), i.to_string(), "settings".to_string()], settings.as_slice()));
            }
        }
    }
    out
}

/// `literalValues(node)`: a JSON object's literal properties as plain values. Other values
/// become null, so they never read as a missing `step`.
pub fn literal_values(children: &[JsonNode<'_>]) -> Value {
    let mut values = Object::new();
    for property in children {
        let JsonNode::Property { key, value, .. } = property else { continue };
        let key = match &**key {
            JsonNode::Identifier { value, .. } => value.to_string(),
            _ => continue,
        };
        let v = match &**value {
            JsonNode::Literal { value: Literal::String(s), .. } => Value::String(s.to_string()),
            JsonNode::Literal { value: Literal::Number(n), .. } => Value::Number(*n),
            JsonNode::Literal { value: Literal::Bool(b), .. } => Value::Bool(*b),
            _ => Value::Null,
        };
        values.insert(key, v);
    }
    Value::Object(values)
}

/// A property's value node, by key.
pub fn property<'n, 's>(children: &'n [JsonNode<'s>], key: &str) -> Option<&'n JsonNode<'s>> {
    children.iter().find(|p| p.property_key() == Some(key)).and_then(|p| match p {
        JsonNode::Property { value, .. } => Some(&**value),
        _ => None,
    })
}

/// `${n}`.
pub fn js(n: f64) -> String {
    number_to_string(n)
}
