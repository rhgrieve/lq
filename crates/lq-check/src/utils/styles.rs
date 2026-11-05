//! `utils/styles.ts`.
//!
//! `extractCSSClassNames` is an approximation of `postcss-safe-parser` plus
//! `postcss-selector-parser`: a tolerant scan for rule selectors, not a CSS parser.
//! Verified only by upstream's unit tests ported below; a check depending on exotic
//! selectors should compare against the oracle.

use crate::check_utils::valued_attribute;
use crate::fs::FileSystem;
use crate::visitor::visit;
use lq_json::js::is_js_whitespace;
use lq_syntax::Node;
use std::collections::BTreeSet;

/// Class names in one selector, as `selectorParser` walks them.
fn classes_in_selector(selector: &str, out: &mut BTreeSet<String>) {
    let chars: Vec<char> = selector.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '[' => {
                // Attribute selector: skip to the closing bracket, strings included.
                let mut quote = None;
                i += 1;
                while i < chars.len() {
                    let c = chars[i];
                    match quote {
                        Some(q) if c == q => quote = None,
                        Some(_) => {}
                        None if c == '"' || c == '\'' => quote = Some(c),
                        None if c == ']' => break,
                        None => {}
                    }
                    i += 1;
                }
                i += 1;
            }
            '"' | '\'' => {
                let q = chars[i];
                i += 1;
                while i < chars.len() && chars[i] != q {
                    i += 1;
                }
                i += 1;
            }
            '.' => {
                i += 1;
                let mut name = String::new();
                while i < chars.len() {
                    let c = chars[i];
                    if c == '\\' && i + 1 < chars.len() {
                        name.push(chars[i + 1]);
                        i += 2;
                        continue;
                    }
                    if c.is_alphanumeric() || c == '-' || c == '_' || !c.is_ascii() {
                        name.push(c);
                        i += 1;
                    } else {
                        break;
                    }
                }
                if !name.is_empty() {
                    out.insert(name);
                }
            }
            _ => i += 1,
        }
    }
}

/// `extractCSSClassNames(css)`.
pub fn extract_css_class_names(css: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let bytes = css.as_bytes();
    let mut prelude = String::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                match css[i + 2..].find("*/") {
                    Some(end) => i += end + 4,
                    None => break,
                }
                continue;
            }
            b'"' | b'\'' => {
                let q = bytes[i];
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != q {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i = (i + 1).min(bytes.len());
                prelude.push_str(&css[start..i]);
                continue;
            }
            b'{' => {
                let selector = prelude.trim_matches(is_js_whitespace);
                if !selector.starts_with('@') {
                    classes_in_selector(selector, &mut out);
                }
                prelude.clear();
            }
            b'}' | b';' => prelude.clear(),
            _ => {
                let w = css[i..].chars().next().map_or(1, char::len_utf8);
                prelude.push_str(&css[i..i + w]);
                i += w;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// `extractCSSClassesFromLiquidAST(ast)`.
pub fn css_classes_from_liquid_ast<'a>(root: &'a Node<'a>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    visit(root, &mut |node, _| {
        if let Node::LiquidRawTag(t) = node {
            if t.name == "stylesheet" {
                if let Node::RawMarkup(b) = &t.body {
                    out.extend(extract_css_class_names(b.value));
                }
            }
        }
    });
    out
}

/// `/\{%-?\s*stylesheet/.test(source)`.
fn may_have_stylesheet(source: &str) -> bool {
    let mut search = 0;
    while let Some(i) = source[search..].find("{%") {
        let mut rest = &source[search + i + 2..];
        rest = rest.strip_prefix('-').unwrap_or(rest);
        if rest.trim_start_matches(is_js_whitespace).starts_with("stylesheet") {
            return true;
        }
        search += i + 2;
    }
    false
}

/// `extractCSSClassesFromLiquidUri(uri, fs)`.
pub fn css_classes_from_liquid_uri(uri: &str, fs: &dyn FileSystem) -> BTreeSet<String> {
    let Ok(source) = fs.read_file(uri) else { return BTreeSet::new() };
    if !may_have_stylesheet(&source) {
        return BTreeSet::new();
    }
    match lq_syntax::parse_strict(&source) {
        Ok(doc) => css_classes_from_liquid_ast(&doc.root),
        Err(_) => BTreeSet::new(),
    }
}

/// `extractCSSClassesFromAssetUri(uri, fs)`.
pub fn css_classes_from_asset_uri(uri: &str, fs: &dyn FileSystem) -> BTreeSet<String> {
    fs.read_file(uri).map_or_else(|_| BTreeSet::new(), |s| extract_css_class_names(&s))
}

/// A class used in markup. Offsets in bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsedClass {
    pub class_name: String,
    pub start: u32,
    pub end: u32,
}

/// `collectUsedClasses(attributes, usedClasses)`.
pub fn collect_used_classes(attributes: &[Node<'_>], used: &mut Vec<UsedClass>) {
    for attr in attributes {
        let Some(a) = valued_attribute(attr) else { continue };
        let is_class = match a.name.as_slice() {
            [Node::TextNode(t)] => t.value.eq_ignore_ascii_case("class"),
            _ => false,
        };
        if !is_class {
            continue;
        }
        for (i, value_node) in a.value.iter().enumerate() {
            let Node::TextNode(text) = value_node else { continue };
            let value: &str = &text.value;
            let base = text.position.start;
            let prev_liquid = i > 0 && !matches!(a.value[i - 1], Node::TextNode(_));
            let next_liquid = i + 1 < a.value.len() && !matches!(a.value[i + 1], Node::TextNode(_));
            let mut tokens = Vec::new();
            let mut start = None;
            for (j, c) in value.char_indices() {
                if is_js_whitespace(c) {
                    if let Some(s) = start.take() {
                        tokens.push((s, j));
                    }
                } else if start.is_none() {
                    start = Some(j);
                }
            }
            if let Some(s) = start {
                tokens.push((s, value.len()));
            }
            if tokens.is_empty() {
                continue;
            }
            if prev_liquid && !value.chars().next().is_some_and(is_js_whitespace) {
                tokens.remove(0);
            }
            if next_liquid && !value.chars().next_back().is_some_and(is_js_whitespace) {
                tokens.pop();
            }
            used.extend(tokens.into_iter().map(|(s, e)| UsedClass {
                class_name: value[s..e].to_string(),
                start: base + s as u32,
                end: base + e as u32,
            }));
        }
    }
}

/// `collectUsedClassesFromSvg(attributes, bodyValue, bodyStartOffset, usedClasses)`.
pub fn collect_used_classes_from_svg(attributes: &[Node<'_>], body: &str, body_start: u32, used: &mut Vec<UsedClass>) {
    collect_used_classes(attributes, used);
    let Ok(doc) = lq_syntax::parse_strict(body) else { return };
    let mut inner = Vec::new();
    visit(&doc.root, &mut |node, _| match node {
        Node::HtmlElement(n) => collect_used_classes(&n.attributes, &mut inner),
        Node::HtmlVoidElement(n) => collect_used_classes(&n.attributes, &mut inner),
        Node::HtmlSelfClosingElement(n) => collect_used_classes(&n.attributes, &mut inner),
        _ => {}
    });
    used.extend(inner.into_iter().map(|c| UsedClass {
        class_name: c.class_name,
        start: c.start + body_start,
        end: c.end + body_start,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(css: &str) -> Vec<String> {
        extract_css_class_names(css).into_iter().collect()
    }

    #[test]
    fn upstream_spec_cases() {
        assert_eq!(classes(".my-class { color: red; }"), ["my-class"]);
        assert_eq!(classes(".a .b { color: red; }"), ["a", "b"]);
        assert_eq!(classes(".a, .b { color: red; }"), ["a", "b"]);
        assert_eq!(classes(".parent > .child + .sibling ~ .cousin { color: red; }"), ["child", "cousin", "parent", "sibling"]);
        assert_eq!(classes(".btn:hover { color: red; } .link::before { content: \"\"; }"), ["btn", "link"]);
        assert_eq!(classes("@media (min-width: 768px) { .responsive { display: block; } }"), ["responsive"]);
        assert_eq!(classes("/* .commented-out { color: red; } */ .real { color: blue; }"), ["real"]);
        assert!(classes("this is {{ not }} valid css {{{").is_empty());
        assert!(classes("").is_empty());
        assert_eq!(classes(".my-class_name { color: red; } ._private { color: blue; }"), ["_private", "my-class_name"]);
        assert_eq!(classes(".a.b { color: red; }"), ["a", "b"]);
    }
}
