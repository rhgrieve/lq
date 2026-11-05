//! `MatchingTranslations`: a cross-file JSON check with a fix and a suggestion.

use crate::api::*;
use lq_json::validator::JsError;
use lq_json::{JsonNode, Literal, Loc, Value};
use std::collections::{HashMap, HashSet};

pub const IMPL: CheckImpl = CheckImpl::Json {
    create,
    enter: JsonMask::of(&[JsonKind::Property]),
    exit: JsonMask::NONE,
};

const PLURALIZATION_KEYS: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];

fn create<'a, 'f>(cx: &JsonCx<'a, 'f>) -> Option<Box<dyn JsonCheck<'a> + 'a>> {
    let uri = cx.uri();
    let is_locale_file = cx.to_relative_path(uri).starts_with("locales/");
    let is_default = uri.ends_with(".default.json") || uri.ends_with(".default.schema.json");
    let Ok(ast) = cx.file().json_ast() else { return None };
    if !is_locale_file || is_default {
        return None;
    }
    Some(Box::new(MatchingTranslations {
        ast,
        is_schema_file: uri.ends_with(".schema.json"),
        default_translations: HashSet::new(),
        missing: Vec::new(),
        missing_index: HashMap::new(),
        nodes_by_path: Vec::new(),
        node_index: HashMap::new(),
    }))
}

struct MatchingTranslations<'a> {
    ast: &'a JsonNode<'a>,
    is_schema_file: bool,
    default_translations: HashSet<String>,
    /// A `Set` in insertion order: `None` once deleted.
    missing: Vec<Option<String>>,
    missing_index: HashMap<String, usize>,
    /// A `Map` in insertion order.
    nodes_by_path: Vec<(String, Loc)>,
    node_index: HashMap<String, usize>,
}

/// `jsonPaths(json)`. `Err` where upstream's `Object.keys` throws on `null`,
/// or `undefined` at the top.
fn json_paths(json: &Value, prefix: &str, out: &mut Vec<String>) -> Result<(), JsError> {
    let mut each = |key: String, value: &Value| -> Result<(), JsError> {
        let path = if prefix.is_empty() { key } else { format!("{prefix}.{key}") };
        match value {
            Value::Object(_) | Value::Array(_) => json_paths(value, &path, out),
            Value::Null => Err(null_to_object()),
            _ => {
                out.push(path);
                Ok(())
            }
        }
    };
    match json {
        Value::Object(o) => {
            for (k, v) in o.iter() {
                each(k.to_string(), v)?;
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                each(i.to_string(), v)?;
            }
        }
        Value::String(s) => {
            for (i, c) in s.chars().enumerate() {
                each(i.to_string(), &Value::String(c.to_string()))?;
            }
        }
        Value::Null => return Err(null_to_object()),
        Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn null_to_object() -> JsError {
    JsError { name: "TypeError".into(), message: "Cannot convert undefined or null to object".into() }
}

fn is_external_path(path: &str) -> bool {
    path.starts_with("shopify.") || path.starts_with("customer_accounts.")
}

fn is_pluralization_path(path: &str) -> bool {
    PLURALIZATION_KEYS.iter().any(|k| path.ends_with(k))
}

fn common_parts(a: &[&str], b: &[&str]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

impl MatchingTranslations<'_> {
    /// `closestTranslationKey(translationKey)`.
    fn closest_loc(&self, key: &str) -> Loc {
        let key_parts: Vec<&str> = key.split('.').collect();
        let mut closest: Option<usize> = None;
        let mut max_common = 0;
        for (i, (path, _)) in self.nodes_by_path.iter().enumerate() {
            let parts: Vec<&str> = path.split('.').collect();
            let common = common_parts(&parts, &key_parts);
            if common > max_common {
                max_common = common;
                closest = Some(i);
            }
        }
        closest.map_or(self.ast.loc(), |i| self.nodes_by_path[i].1)
    }
}

impl<'a> JsonCheck<'a> for MatchingTranslations<'a> {
    fn on_code_path_start(&mut self, cx: &JsonCx<'a, '_>) {
        let translations = if self.is_schema_file {
            cx.default_schema_translations()
        } else {
            cx.default_translations()
        };
        let Some(translations) = translations else {
            cx.fail(null_to_object());
            return;
        };
        let mut paths = Vec::new();
        if let Err(e) = json_paths(translations, "", &mut paths) {
            cx.fail(e);
            return;
        }
        for path in paths {
            if !self.missing_index.contains_key(&path) {
                self.missing_index.insert(path.clone(), self.missing.len());
                self.missing.push(Some(path.clone()));
            }
            self.default_translations.insert(path);
        }
    }

    fn enter(&mut self, node: &'a JsonNode<'a>, ancestors: &[&'a JsonNode<'a>], cx: &JsonCx<'a, '_>) {
        let JsonNode::Property { value, loc, .. } = node else { return };
        let mut path = String::new();
        for key in ancestors.iter().filter_map(|n| n.property_key()) {
            path.push_str(key);
            path.push('.');
        }
        let key = node.property_key().unwrap_or("");
        path.push_str(key);

        match self.node_index.get(&path) {
            Some(&i) => self.nodes_by_path[i].1 = *loc,
            None => {
                self.node_index.insert(path.clone(), self.nodes_by_path.len());
                self.nodes_by_path.push((path.clone(), *loc));
            }
        }

        if self.default_translations.is_empty()
            || PLURALIZATION_KEYS.contains(&key)
            || !matches!(**value, JsonNode::Literal { value: Literal::String(_) | Literal::Number(_) | Literal::Bool(_) | Literal::Null, .. })
            || is_external_path(&path)
        {
            return;
        }
        if self.default_translations.contains(&path) {
            if let Some(&i) = self.missing_index.get(&path) {
                self.missing[i] = None;
            }
            return;
        }
        cx.report(
            JsonProblem::new(format!("A default translation for '{path}' does not exist"), loc.start, loc.end)
                .suggest("Delete unneeded translation key", |c| c.remove(path.clone())),
        );
    }

    fn on_code_path_end(&mut self, cx: &JsonCx<'a, '_>) {
        for path in self.missing.iter().flatten() {
            if is_pluralization_path(path) || is_external_path(path) {
                continue;
            }
            let loc = self.closest_loc(path);
            cx.report(
                JsonProblem::new(format!("The translation for '{path}' is missing"), loc.start, loc.end)
                    .fix(|c| c.add(path.clone(), Value::String("TODO".into()))),
            );
        }
    }
}
