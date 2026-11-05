//! `valid-visible-if/visible-if-utils.ts`.

use crate::visitor::visit;
use lq_syntax::{Markup, Node};

pub const ADJUSTED_PREFIX: &str = "{% if ";
pub const ADJUSTED_SUFFIX: &str = " %}{% endif %}";
/// `offsetAdjust` = `'{{'.length - adjustedPrefix.length`.
pub const OFFSET_ADJUST: i64 = 2 - 6;

/// A `VariableLookup` found in the expression, with positions in the adjusted string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpressionLookup {
    pub name: Option<String>,
    /// `Err` holds the type of a non-String lookup, which `getNormalizedLookups` rejects.
    pub lookups: Vec<Result<String, String>>,
    pub start: u32,
    pub end: u32,
    /// `start` and `end` in UTF-16 units: what JS adds to an offset in the source.
    pub start_units: u32,
    pub end_units: u32,
}

/// The result of `getVariableLookupsInExpression`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExpressionLookups {
    /// `null`: a literal `true`/`false`.
    None,
    Warning(String),
    Lookups(Vec<ExpressionLookup>),
}

/// `variableExpressionMatcher.exec(expression)`: `/{{(.+?)}}/`, as `(group, match start)`.
/// The start, in bytes into `expression`, is `match.index`.
pub fn match_expression(expression: &str) -> Option<(&str, usize)> {
    let mut search = 0;
    while let Some(i) = expression[search..].find("{{") {
        let open = search + i;
        let inner_start = open + 2;
        let first = expression[inner_start..].chars().next();
        let mut pos = inner_start;
        // `.+?`: at least one non-line-terminator char, lazily, up to `}}`.
        if let Some(c) = first {
            if !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
                pos += c.len_utf8();
                loop {
                    if expression[pos..].starts_with("}}") {
                        return Some((&expression[inner_start..pos], open));
                    }
                    match expression[pos..].chars().next() {
                        Some(c) if !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') => pos += c.len_utf8(),
                        _ => break,
                    }
                }
            }
        }
        search = inner_start - 1;
    }
    None
}

/// `match.index` in UTF-16 units: what [`Divergence`](crate::api::Divergence)`::VisibleIfMatchIndex`
/// adds to a position. Upstream's positions assume the expression starts the string.
pub fn match_index_units(expression: &str) -> u32 {
    match_expression(expression).map_or(0, |(_, index)| lq_json::js::utf16_len(&expression[..index]) as u32)
}

/// `getVariableLookupsInExpression(expression)`.
pub fn variable_lookups_in_expression(expression: &str) -> ExpressionLookups {
    let Some((unwrapped, _)) = match_expression(expression) else {
        return ExpressionLookups::Warning(
            "Invalid visible_if expression. It should take the form \"{{ <expression> }}\".".into(),
        );
    };
    let adjusted = format!("{ADJUSTED_PREFIX}{unwrapped}{ADJUSTED_SUFFIX}");
    let doc = match lq_syntax::parse_strict(&adjusted) {
        Ok(d) => d,
        Err(e) if e.name == "LiquidHTMLParsingError" => {
            return ExpressionLookups::Warning("Syntax error: cannot parse visible_if expression.".into());
        }
        Err(e) => return ExpressionLookups::Warning(format!("{}: {}", e.name, e.message)),
    };
    let children = doc.children();
    if children.len() != 1 {
        return ExpressionLookups::Warning("Error: Unexpected child count for DocumentNode".into());
    }
    let Node::LiquidTag(tag) = &children[0] else {
        return ExpressionLookups::Warning("Error: Expected DocumentNode to contain 'if' tag".into());
    };
    if tag.name != "if" {
        return ExpressionLookups::Warning("Error: Expected DocumentNode to contain 'if' tag".into());
    }
    let Markup::Node(expression_node) = &tag.markup else {
        return ExpressionLookups::Warning("Syntax error: cannot parse visible_if expression.".into());
    };
    if let Node::LiquidLiteral(l) = &**expression_node {
        if l.keyword == "true" || l.keyword == "false" {
            return ExpressionLookups::None;
        }
    }
    let mut vars = Vec::new();
    visit(&children[0], &mut |node, _| {
        if let Node::VariableLookup(l) = node {
            vars.push(ExpressionLookup {
                name: l.name.map(String::from),
                lookups: l
                    .lookups
                    .iter()
                    .map(|x| match x {
                        Node::String(s) => Ok(s.value.to_string()),
                        other => Err(other.node_type().to_string()),
                    })
                    .collect(),
                start: node.position().start,
                end: node.position().end,
                start_units: lq_json::js::utf16_len(&adjusted[..node.position().start as usize]) as u32,
                end_units: lq_json::js::utf16_len(&adjusted[..node.position().end as usize]) as u32,
            });
        }
    });
    if vars.is_empty() {
        return ExpressionLookups::Warning(
            "visible_if expression contains no references to any settings. This is likely an error.".into(),
        );
    }
    ExpressionLookups::Lookups(vars)
}

/// `validateLookup(lookup, vars)`, over a tree of settings: `vars` holds the dotted
/// variables, a leaf being `true`. `Err` is the thrown non-String lookup error's type.
///
/// `vars` is a plain JS object: `Object.prototype` members resolve to functions, which act
/// as propertyless namespaces. A `null` name looks up `"null"` and joins as an empty string.
///
/// With `own_only` ([`Divergence`](crate::api::Divergence)`::PrototypeLookup`) only own properties count.
pub fn validate_lookup(lookup: &ExpressionLookup, vars: &lq_json::Value, own_only: bool) -> Result<Option<String>, String> {
    let mut normalized: Vec<Option<&str>> = vec![lookup.name.as_deref()];
    for l in &lookup.lookups {
        normalized.push(Some(l.as_ref().map_err(String::clone)?));
    }
    let mut popped: Vec<&str> = Vec::new();
    let mut scope = Some(vars);
    let n = normalized.len();
    for (i, segment) in normalized.iter().enumerate() {
        popped.push(segment.unwrap_or(""));
        let key = segment.unwrap_or("null");
        let joined = || popped.join(".");
        match scope.and_then(|s| s.get(key)).filter(|v| v.is_truthy()) {
            Some(next) if next.as_bool().is_some() => {
                if i + 1 < n {
                    return Ok(Some(format!(
                        "Invalid variable: \"{}\" refers to a variable, but is being used here as a namespace.",
                        joined()
                    )));
                }
                return Ok(None);
            }
            Some(next) => scope = Some(next),
            None if !own_only && crate::checks::shared_schema::OBJECT_PROTOTYPE_KEYS.contains(&key) => scope = None,
            None => return Ok(Some(format!("Invalid variable: \"{}\" was not found.", joined()))),
        }
    }
    Ok(Some(format!(
        "Invalid variable: \"{}\" refers to a namespace, but is being used here as a variable.",
        popped.join(".")
    )))
}

/// The thrown error of `getNormalizedLookups`.
pub fn lookup_error(ty: &str) -> lq_json::validator::JsError {
    lq_json::validator::JsError { name: "Error".into(), message: format!("Expected lookups to be String nodes: {ty}") }
}

/// `Object.fromEntries(ids.map((s) => [s, true]))`.
pub fn settings_scope<S: AsRef<str>>(ids: impl Iterator<Item = S>) -> lq_json::Value {
    let mut out = lq_json::Object::new();
    for id in ids {
        out.insert(id.as_ref(), lq_json::Value::Bool(true));
    }
    lq_json::Value::Object(out)
}

/// `getGlobalSettings(context)`: the setting ids in `config/settings_schema.json`. Upstream's
/// `try` keeps what it collected before something throws: a primitive group or `null` setting.
pub fn global_settings<C>(cx: &crate::api::Cx<'_, '_, C>) -> Vec<lq_json::Value> {
    let mut out = Vec::new();
    let Ok(text) = cx.fs().read_file(&cx.to_uri("config/settings_schema.json")) else { return out };
    let Some(lq_json::Value::Array(groups)) = lq_json::parse_json(&text) else { return out };
    for group in &groups {
        if !matches!(group, lq_json::Value::Object(_) | lq_json::Value::Array(_)) {
            break;
        }
        if let Some(settings) = group.get("settings").and_then(lq_json::Value::as_array) {
            if settings.iter().any(lq_json::Value::is_null) {
                break;
            }
            out.extend(settings.iter().filter_map(|s| s.get("id")).filter(|id| id.is_truthy()).cloned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(expression: &str) -> ExpressionLookup {
        match variable_lookups_in_expression(expression) {
            ExpressionLookups::Lookups(mut l) => l.remove(0),
            other => panic!("{other:?}"),
        }
    }

    fn vars() -> lq_json::Value {
        lq_json::parse_json(r#"{"settings":{"on":true}}"#).unwrap()
    }

    #[test]
    fn matcher_is_the_regex() {
        assert_eq!(match_expression("{{ a }}"), Some((" a ", 0)));
        assert_eq!(match_expression("x {{{}}}"), Some(("{", 2)));
        assert_eq!(match_expression("{{}}"), None);
        assert_eq!(match_expression("{{\n}} {{ b }}"), Some((" b ", 6)));
    }

    #[test]
    fn classifies_expressions() {
        assert_eq!(variable_lookups_in_expression("{{ true }}"), ExpressionLookups::None);
        let warning = |s: &str| ExpressionLookups::Warning(s.to_string());
        assert_eq!(
            variable_lookups_in_expression("nope"),
            warning("Invalid visible_if expression. It should take the form \"{{ <expression> }}\".")
        );
        assert_eq!(variable_lookups_in_expression("{{ %% }}"), warning("Syntax error: cannot parse visible_if expression."));
        assert_eq!(
            variable_lookups_in_expression("{{ 1 == 1 }}"),
            warning("visible_if expression contains no references to any settings. This is likely an error.")
        );
    }

    #[test]
    fn validates_lookups() {
        let check = |e: &str| validate_lookup(&lookup(e), &vars(), false).unwrap();
        assert_eq!(check("{{ settings.on }}"), None);
        assert_eq!(check("{{ settings.no }}").unwrap(), "Invalid variable: \"settings.no\" was not found.");
        assert_eq!(
            check("{{ settings.on.x }}").unwrap(),
            "Invalid variable: \"settings.on\" refers to a variable, but is being used here as a namespace."
        );
        assert_eq!(check("{{ settings }}").unwrap(), "Invalid variable: \"settings\" refers to a namespace, but is being used here as a variable.");
        // Plain JS objects have prototypes, and `null` joins as an empty string.
        assert_eq!(check("{{ settings.toString }}").unwrap(), "Invalid variable: \"settings.toString\" refers to a namespace, but is being used here as a variable.");
        assert_eq!(check("{{ ['x'] }}").unwrap(), "Invalid variable: \"\" was not found.");
        assert!(validate_lookup(&lookup("{{ settings[x] }}"), &vars(), false).is_err());
        assert_eq!(
            validate_lookup(&lookup("{{ settings.toString }}"), &vars(), true).unwrap().unwrap(),
            "Invalid variable: \"settings.toString\" was not found."
        );
    }

    #[test]
    fn lookup_positions_in_units() {
        let l = lookup("{{ '😀' == a }}");
        assert_eq!(l.end - l.start, 1);
        assert_eq!(l.start - l.start_units, 2);
    }
}
