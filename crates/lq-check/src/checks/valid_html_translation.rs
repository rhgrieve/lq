//! `ValidHTMLTranslation`: strings in `locales/` that hold HTML must parse.

use crate::api::*;
use lq_json::ast::Literal;
use lq_json::JsonNode;

pub const IMPL: CheckImpl =
    CheckImpl::Json { create, enter: JsonMask::of(&[JsonKind::Literal]), exit: JsonMask::NONE };

fn create<'a, 'f>(cx: &JsonCx<'a, 'f>) -> Option<Box<dyn JsonCheck<'a> + 'a>> {
    if !cx.to_relative_path(cx.uri()).starts_with("locales/") {
        return None;
    }
    Some(Box::new(ValidHtmlTranslation))
}

struct ValidHtmlTranslation;

impl<'a> JsonCheck<'a> for ValidHtmlTranslation {
    fn enter(&mut self, node: &'a JsonNode<'a>, _: &[&'a JsonNode<'a>], cx: &JsonCx<'a, '_>) {
        let JsonNode::Literal { value: Literal::String(value), loc } = node else { return };
        if !has_html(value) {
            return;
        }
        if let Err(error) = lq_syntax::parse_strict(value) {
            cx.report(JsonProblem::new(format!("{error}."), loc.start, loc.end));
        }
    }
}

/// `/<[^>]+>/.test(s)`.
fn has_html(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while let Some(lt) = b[i..].iter().position(|&c| c == b'<') {
        let lt = i + lt;
        // At least one char other than `>`, then a `>`.
        if b.get(lt + 1).is_some_and(|&c| c != b'>') && b[lt + 2..].contains(&b'>') {
            return true;
        }
        i = lt + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::has_html;

    #[test]
    fn html_regex() {
        for (s, want) in [
            ("", false),
            ("<", false),
            ("<>", false),
            ("<b", false),
            ("a < b", false),
            ("<b>", true),
            ("<>x<a>", true),
            ("<\n>", true),
            ("<><", false),
            ("<\u{e9}>", true),
        ] {
            assert_eq!(has_html(s), want, "{s:?}");
        }
    }
}
