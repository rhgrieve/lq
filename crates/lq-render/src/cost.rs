//! The cost model: operation counts times per-operation coefficients, in microseconds.
//!
//! Operation names are plain strings so the model file stays readable and calibration can fit
//! any of them: `node`, `lookup`, `iter`, `output_byte`, `filter:<name>`, `db:<type>.<field>`, ...
//! A missing name falls back to its prefix default (`filter:*`, `db:*`), then to 0.

use std::collections::HashMap;

/// Built-in coefficients. These are estimates, not measurements: `lq profile calibrate` replaces
/// them with values fitted to real `shopify theme profile` output.
pub const DEFAULT_COSTS: &str = include_str!("../../../data/profile-costs.json");

#[derive(Clone, Debug)]
pub struct CostModel {
    pub coefficients: HashMap<String, f64>,
    /// Database-backed properties, as `type.field` or `type.*`. Reading one costs `db:<key>` the
    /// first time per object, and a plain `lookup` after that, as SFR caches drops per request.
    pub db_fields: Vec<String>,
    pub calibrated: bool,
    pub source: String,
}

impl CostModel {
    pub fn builtin() -> CostModel {
        CostModel::from_json(DEFAULT_COSTS, "built-in estimates").expect("built-in cost model parses")
    }

    pub fn from_json(text: &str, source: &str) -> Result<CostModel, String> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let mut coefficients = HashMap::new();
        if let Some(obj) = v.get("coefficients").and_then(|c| c.as_object()) {
            for (k, x) in obj {
                if let Some(f) = x.as_f64() {
                    coefficients.insert(k.clone(), f);
                }
            }
        }
        let db_fields = v
            .get("db_fields")
            .and_then(|d| d.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let calibrated = v.get("calibrated").and_then(|c| c.as_bool()).unwrap_or(false);
        Ok(CostModel { coefficients, db_fields, calibrated, source: source.to_string() })
    }

    /// Overlays another model's coefficients, as a calibrated file over the defaults.
    pub fn overlay(&mut self, other: CostModel) {
        self.coefficients.extend(other.coefficients);
        if !other.db_fields.is_empty() {
            self.db_fields = other.db_fields;
        }
        self.calibrated |= other.calibrated;
        self.source = other.source;
    }

    pub fn coefficient(&self, op: &str) -> f64 {
        if let Some(c) = self.coefficients.get(op) {
            return *c;
        }
        if let Some((prefix, _)) = op.split_once(':') {
            if let Some(c) = self.coefficients.get(&format!("{prefix}:*")) {
                return *c;
            }
        }
        0.0
    }

    /// The `db:` op for reading `field` on an object of type `ty`, if it's database-backed.
    pub fn db_op(&self, ty: &str, field: &str) -> Option<String> {
        if ty.is_empty() {
            return None;
        }
        for f in &self.db_fields {
            let Some((t, name)) = f.split_once('.') else { continue };
            if t == ty && (name == field || name == "*") {
                return Some(format!("db:{ty}.{name}"));
            }
        }
        None
    }

    pub fn to_json(&self) -> String {
        let mut keys: Vec<_> = self.coefficients.iter().collect();
        keys.sort_by(|a, b| a.0.cmp(b.0));
        let coefficients: serde_json::Map<String, serde_json::Value> = keys
            .into_iter()
            .map(|(k, v)| (k.clone(), serde_json::json!((v * 1000.0).round() / 1000.0)))
            .collect();
        let v = serde_json::json!({
            "calibrated": self.calibrated,
            "unit": "microseconds",
            "coefficients": coefficients,
            "db_fields": self.db_fields,
        });
        serde_json::to_string_pretty(&v).unwrap() + "\n"
    }
}

/// Interns op names so frames can store small `(id, count)` lists.
#[derive(Default, Debug, Clone)]
pub struct Ops {
    pub names: Vec<String>,
    index: HashMap<String, u32>,
}

impl Ops {
    pub fn id(&mut self, name: &str) -> u32 {
        if let Some(&i) = self.index.get(name) {
            return i;
        }
        let i = self.names.len() as u32;
        self.names.push(name.to_string());
        self.index.insert(name.to_string(), i);
        i
    }

    pub fn name(&self, id: u32) -> &str {
        &self.names[id as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_fallback() {
        let m = CostModel::builtin();
        assert!(m.coefficient("filter:some_unknown_filter") > 0.0);
        assert!(m.coefficient("node") > 0.0);
        assert!(m.db_op("product", "metafields").is_some());
        assert!(m.db_op("product", "title").is_none());
    }
}
