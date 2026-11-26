//! `valid-select-default/index.ts`'s helpers, shared by both `ValidSelectDefault` variants.

use lq_json::js::{number_to_string, quote};
use lq_json::{JsonNode, Literal};

/// A scalar literal (`isScalar`), as `String(value)` and `JSON.stringify(value)` give it.
pub struct Scalar {
    pub string: String,
    pub json: String,
}

fn scalar(value: &Literal<'_>) -> Option<Scalar> {
    Some(match value {
        Literal::String(s) => Scalar { string: s.to_string(), json: quote(s) },
        Literal::Number(n) => Scalar { string: number_to_string(*n), json: number_to_string(*n) },
        Literal::Bool(b) => Scalar { string: b.to_string(), json: b.to_string() },
        Literal::Null => return None,
    })
}

/// A property's value node, by key: `object.children.find((p) => p.key.value === key)?.value`.
pub fn value_of<'n, 's>(object: &'n JsonNode<'s>, key: &str) -> Option<&'n JsonNode<'s>> {
    let JsonNode::Object { children, .. } = object else { return None };
    children.iter().find(|p| p.property_key() == Some(key)).and_then(JsonNode::property_value)
}

/// `literal(object, key)`.
fn literal<'n, 's>(object: &'n JsonNode<'s>, key: &str) -> Option<&'n Literal<'s>> {
    match value_of(object, key) {
        Some(JsonNode::Literal { value, .. }) => Some(value),
        _ => None,
    }
}

fn literal_str<'n>(object: &'n JsonNode<'_>, key: &str) -> Option<&'n str> {
    match literal(object, key) {
        Some(Literal::String(s)) => Some(s),
        _ => None,
    }
}

/// `choiceOptions(setting)`: the option values of a select or radio setting.
fn choice_options(setting: &JsonNode<'_>) -> Option<Vec<String>> {
    if !matches!(literal_str(setting, "type"), Some("select" | "radio")) {
        return None;
    }
    let Some(JsonNode::Array { children, .. }) = value_of(setting, "options") else { return None };
    Some(children.iter().filter_map(|option| literal(option, "value").and_then(scalar)).map(|s| s.string).collect())
}

fn list(options: &[String]) -> String {
    options.iter().map(|o| format!("\"{o}\"")).collect::<Vec<_>>().join(", ")
}

/// `checkDefaults(settings, report)`: each select/radio setting whose default isn't one of its
/// options.
pub fn check_defaults(settings: Option<&JsonNode<'_>>, report: &mut dyn FnMut(String, &JsonNode<'_>)) {
    let Some(JsonNode::Array { children, .. }) = settings else { return };
    for setting in children {
        if !matches!(setting, JsonNode::Object { .. }) {
            continue;
        }
        let (Some(id), Some(options)) = (literal_str(setting, "id"), choice_options(setting)) else { continue };
        let Some(default_node @ JsonNode::Literal { value, .. }) = value_of(setting, "default") else { continue };
        let Some(value) = scalar(value) else { continue };
        if !options.contains(&value.string) {
            report(
                format!("Default value {} for setting \"{id}\" is not one of the valid options: {}.", value.json, list(&options)),
                default_node,
            );
        }
    }
}

/// `checkValues(values, settings, report)`: each value in `values`, an object of setting values
/// by id, that isn't one of the options of the select/radio setting it sets in `settings`.
pub fn check_values(values: Option<&JsonNode<'_>>, settings: Option<&JsonNode<'_>>, report: &mut dyn FnMut(String, &JsonNode<'_>)) {
    let (Some(JsonNode::Object { children: values, .. }), Some(JsonNode::Array { children: settings, .. })) = (values, settings) else {
        return;
    };
    for property in values {
        let Some(id) = property.property_key() else { continue };
        let setting = settings.iter().find(|s| matches!(s, JsonNode::Object { .. }) && literal_str(s, "id") == Some(id));
        let Some(options) = setting.and_then(choice_options) else { continue };
        let Some(value_node @ JsonNode::Literal { value, .. }) = property.property_value() else { continue };
        let Some(value) = scalar(value) else { continue };
        if !options.contains(&value.string) {
            report(
                format!("Value {} for setting \"{id}\" is not one of the valid options: {}.", value.json, list(&options)),
                value_node,
            );
        }
    }
}

/// `settingsPath(typePath)`: the path of a block's `settings`, from the path of its `type`.
pub fn settings_path(type_path: &[String]) -> Vec<String> {
    let mut p = type_path[..type_path.len().saturating_sub(1)].to_vec();
    p.push("settings".into());
    p
}
