//! `TranslationKeyExists`.

use super::shared_rules::{parsed_schema_body, type_error};
use crate::api::*;
use lq_json::Value;
use lq_syntax::{Node, RawMarkupKind};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidVariable, NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::<TranslationKeyExists<'a>>::default())
}

#[derive(Default)]
struct TranslationKeyExists<'a> {
    nodes: Vec<(&'a str, u32, u32)>,
    /// `schemaLocales`: the parsed schema of the last `schema` tag, whose
    /// `locales[defaultLocale]` it stands for.
    schema: Option<Value>,
}

/// `keyExists(key, pointer)`. `Err` is the `TypeError` of `null.hasOwnProperty`.
///
/// With `null_not_found` ([`Divergence::NullInPath`]), a `null` along the path is not found.
fn key_exists(key: &str, mut pointer: Option<&Value>, null_not_found: bool) -> Result<bool, ()> {
    let mut tokens = key.split('.').peekable();
    while let Some(token) = tokens.next() {
        pointer = match pointer {
            None => return Ok(false),
            Some(Value::Null) if null_not_found => return Ok(false),
            Some(Value::Null) => return Err(()),
            Some(Value::Object(object)) => match object.get(token) {
                Some(value) => Some(value),
                None => return Ok(false),
            },
            Some(Value::Array(items)) => {
                if token == "length" {
                    // A number: only fine as the last token.
                    return Ok(tokens.peek().is_none());
                }
                match lq_json::value::array_index(token).and_then(|i| items.get(i as usize)) {
                    Some(value) => Some(value),
                    None => return Ok(false),
                }
            }
            Some(_) => return Ok(false),
        };
    }
    Ok(true)
}

impl<'a> LiquidCheck<'a> for TranslationKeyExists<'a> {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        match node {
            Node::LiquidVariable(variable) => {
                let Node::String(expression) = &variable.expression else { return };
                let translated =
                    variable.filters.iter().any(|f| matches!(f, Node::LiquidFilter(f) if matches!(f.name, "t" | "translate")));
                if translated {
                    let p = variable.expression.position();
                    self.nodes.push((expression.value, p.start, p.end));
                }
            }
            Node::LiquidRawTag(tag) if tag.name == "schema" => {
                if !matches!(&tag.body, Node::RawMarkup(body) if body.kind == RawMarkupKind::Json) {
                    return;
                }
                if let Some(schema) = parsed_schema_body(cx, tag, "locales") {
                    self.schema = Some(schema);
                }
            }
            _ => {}
        }
    }

    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {
        let translations = cx.default_translations();
        let locale = cx.default_locale();
        let docset = cx.docset();
        let system_count = docset.map_or(0, |d| d.system_translation_count());
        if !translations.is_some_and(Value::is_truthy) && system_count == 0 {
            return;
        }
        let schema_locales = self.schema.as_ref().and_then(|s| s.get("locales")).and_then(|l| l.get(locale));

        for &(key, start, end) in &self.nodes {
            let null_not_found = cx.divergence(Divergence::NullInPath);
            let exists = key_exists(key, translations, null_not_found)
                .and_then(|found| if found { Ok(true) } else { key_exists(key, schema_locales, null_not_found) })
                .map(|found| found || docset.is_some_and(|d| d.has_system_translation(key)));
            match exists {
                Ok(true) => continue,
                Ok(false) => {}
                Err(()) => {
                    cx.fail(type_error("Cannot read properties of null (reading 'hasOwnProperty')"));
                    return;
                }
            }
            let mut message = format!("'{key}' does not have a matching entry in 'locales/{locale}.default.json'");
            if schema_locales.is_some_and(Value::is_truthy) {
                message.push_str(" or '");
                message.push_str(&cx.to_relative_path(cx.uri()));
                message.push('\'');
            }
            cx.report(LiquidProblem::new(message, start, end));
        }
    }
}
