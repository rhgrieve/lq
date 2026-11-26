//! Where a raw tag's body ends: `document/raw-body.ts` on theme-tools `main`
//! (e391d5ca), used by `ParseTarget::Main` only. Each Liquid tag class finds the
//! end of its own body, so the rule belongs to the tag.

use crate::ast::WhitespaceMarker;
use crate::tags::LiquidLine;

/// How a tag is named inside the body: `Leading` reads the name at the start of
/// a `{% %}` token (`BlockBody::FullToken`); `Trailing` reads the last tag in the
/// token and leaves any text before it in the body (`FullTokenPossiblyInvalid`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Delimiter {
    Leading,
    Trailing,
}

/// What an inner tag with the same name does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SameName {
    Nests,
    Ignored,
    Rejected,
}

/// `RawBodyRule` in tag-definitions.ts.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RawBodyRule {
    pub delimiter: Delimiter,
    pub same_name: SameName,
    /// Inner tags whose bodies are skipped whole, each ending by its own rule.
    pub opaque: &'static [&'static str],
    /// Whether the body can close inside `{% liquid %}`.
    pub closes_in_liquid_tag: bool,
}

// `Liquid::Raw` (raw, and liquid_extensions' javascript, schema and stylesheet)
// closes at the last tag of its first `end<name>` token, and cannot close inside
// `{% liquid %}`.
const LIQUID_RAW_BODY: RawBodyRule = RawBodyRule {
    delimiter: Delimiter::Trailing,
    same_name: SameName::Ignored,
    opaque: &[],
    closes_in_liquid_tag: false,
};

// `Liquid::Doc` reads tags like `Liquid::Raw` but rejects a nested `doc`.
const LIQUID_DOC_BODY: RawBodyRule = RawBodyRule { same_name: SameName::Rejected, ..LIQUID_RAW_BODY };

// `Liquid::Comment#parse_body` counts nested comments and skips `raw` bodies.
const LIQUID_COMMENT_BODY: RawBodyRule = RawBodyRule {
    delimiter: Delimiter::Leading,
    same_name: SameName::Nests,
    opaque: &["raw"],
    closes_in_liquid_tag: true,
};

// Shopify's `style` tag is a `Liquid::Block`, so its body nests like any block and
// the raw tags inside it end by their own rules.
const LIQUID_BLOCK_BODY: RawBodyRule = RawBodyRule {
    delimiter: Delimiter::Leading,
    same_name: SameName::Nests,
    opaque: &["comment", "doc", "javascript", "raw", "schema", "stylesheet"],
    closes_in_liquid_tag: true,
};

/// `rawBodyRuleLookup(tagForName)` over the builtin tags: a raw tag's body rule.
pub(crate) fn rule_for(name: &str) -> Option<&'static RawBodyRule> {
    Some(match name {
        "comment" => &LIQUID_COMMENT_BODY,
        "doc" => &LIQUID_DOC_BODY,
        "raw" | "javascript" | "schema" | "stylesheet" => &LIQUID_RAW_BODY,
        "style" => &LIQUID_BLOCK_BODY,
        _ => return None,
    })
}

/// Whether a builtin raw tag parses Liquid in its body (`parseLiquidInBody`).
pub(crate) fn parses_liquid_in_body(name: &str) -> bool {
    matches!(name, "javascript" | "style" | "stylesheet")
}

/// The closing tag of a raw tag's body, as source offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RawBodyEnd {
    pub tag_start: u32,
    pub tag_end: u32,
    pub ws_start: WhitespaceMarker,
    pub ws_end: WhitespaceMarker,
}

/// Ruby's ASCII `\s`.
#[inline]
fn is_ruby_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

#[inline]
fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn word_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && is_word(b[i]) {
        i += 1;
    }
    i
}

/// `FULL_TOKEN.exec(token)?.[1]`: the name a whole `{% … %}` token opens, its
/// leading `#` or word (`BlockBody::FullToken`).
pub(crate) fn liquid_tag_token_name(token: &str) -> Option<&str> {
    let b = token.as_bytes();
    if !b.starts_with(b"{%") || !b.ends_with(b"%}") || b.len() < 4 {
        return None;
    }
    let mut i = 2;
    if b[i] == b'-' {
        i += 1;
    }
    while i < b.len() && is_ruby_ws(b[i]) {
        i += 1;
    }
    let end = if b.get(i) == Some(&b'#') { i + 1 } else { word_end(b, i) };
    // The name can't reach into the closing `%}`.
    if end == i || end > b.len() - 2 {
        return None;
    }
    Some(&token[i..end])
}

/// `TRAILING_TAG_NAME` (`/\{%-?\s*(\w+)/y`) at `at`: the name.
fn trailing_tag_name_at(token: &str, at: usize) -> Option<&str> {
    let b = token.as_bytes();
    let mut i = at + 2;
    if b.get(i) == Some(&b'-') {
        i += 1;
    }
    while i < b.len() && is_ruby_ws(b[i]) {
        i += 1;
    }
    let end = word_end(b, i);
    (end > i).then(|| &token[i..end])
}

/// `tokenTag`: the tag a token names under `delimiter`, and where that tag starts
/// in the token.
fn token_tag(token: &str, delimiter: Delimiter) -> Option<(&str, usize)> {
    if !token.ends_with("%}") {
        return None;
    }
    match delimiter {
        Delimiter::Leading => liquid_tag_token_name(token).map(|name| (name, 0)),
        // Raw accepts the last tag opener in a Liquid token, including one inside
        // an unfinished output.
        Delimiter::Trailing => {
            let b = token.as_bytes();
            let mut limit = token.len() - 3; // lastIndexOf('{%', length - 3)
            loop {
                let offset = memchr::memmem::rfind(&b[..(limit + 2).min(b.len())], b"{%")?;
                if let Some(name) = trailing_tag_name_at(token, offset) {
                    return Some((name, offset));
                }
                if offset == 0 {
                    return None;
                }
                limit = offset - 1;
            }
        }
    }
}

/// `Tokenizer#next_tag_token`: an unterminated `{%` is a token of its own.
fn tag_token_end(source: &[u8], after_open: usize) -> usize {
    match memchr::memmem::find(&source[after_open.min(source.len())..], b"%}") {
        Some(i) => after_open + i + 2,
        None => after_open,
    }
}

/// `Tokenizer#next_variable_token`: a `{%` inside an unfinished `{{` extends it to
/// that tag's `%}`.
fn variable_token_end(source: &[u8], after_open: usize) -> usize {
    let at = |i: usize| source.get(i).copied();
    let mut position = after_open;
    let mut current = at(position);
    position += 1;
    let mut next = current;
    while next.is_some() {
        while current.is_some_and(|c| c != b'}' && c != b'{') {
            current = at(position);
            position += 1;
        }
        if current.is_none() || position >= source.len() {
            return source.len();
        }
        next = at(position);
        position += 1;
        if current == Some(b'}') {
            return if next == Some(b'}') { position } else { position - 1 };
        }
        if current == Some(b'{') && next == Some(b'%') {
            return tag_token_end(source, position);
        }
        current = next;
    }
    source.len()
}

/// The first `{{` or `{%` at or after `from`.
fn next_liquid_open(source: &[u8], from: usize) -> Option<usize> {
    memchr::memchr_iter(b'{', &source[from.min(source.len())..])
        .map(|i| from + i)
        .find(|&i| matches!(source.get(i + 1), Some(b'{') | Some(b'%')))
}

/// The index of a builtin raw tag name, for the caches below.
fn name_index(name: &str) -> Option<usize> {
    ["comment", "doc", "raw", "javascript", "schema", "stylesheet", "style"].iter().position(|&n| n == name)
}

/// `RawBodyEndCache`: answers to earlier `find_raw_body_end` searches in one
/// source. A tolerant parse records an unclosed raw tag and carries on, so it can
/// search for many bodies that run to the end of the document; this keeps each
/// of those searches from reading the rest of the source again (cbe4c7f2,
/// 61cae020).
#[derive(Default)]
pub(crate) struct RawBodyEndCache {
    /// Where each closing name last occurs, by `name_index`.
    last_indexes: [Option<Option<usize>>; 7],
    /// The ends that searches found, by tag and where a search from there starts.
    known: std::collections::HashMap<(usize, usize), Option<RawBodyEnd>>,
}

impl RawBodyEndCache {
    /// Whether `source` has no `end_name` at or after `from`.
    fn absent_from(&mut self, source: &str, tag: usize, end_name: &str, from: usize) -> bool {
        let index = *self.last_indexes[tag]
            .get_or_insert_with(|| memchr::memmem::rfind(source.as_bytes(), end_name.as_bytes()));
        index.is_none_or(|i| i < from)
    }
}

/// `findRawBodyEnd`: where the body of `tag_name` that starts at `body_start` ends
/// under `rule`, reading the source as `Liquid::Tokenizer` tokens. `None` where
/// Liquid reports the tag as never closed, or rejects a tag nested inside it.
///
/// With a `cache` for this source, a search returns early when the closing name
/// doesn't occur after the body, a nesting body reuses what an enclosing search
/// found for it, and a search under a rule that doesn't nest stops where it
/// reaches a token an earlier search read.
pub(crate) fn find_raw_body_end(
    source: &str,
    body_start: usize,
    tag_name: &str,
    rule: &RawBodyRule,
    mut cache: Option<&mut RawBodyEndCache>,
) -> Option<RawBodyEnd> {
    let end_name = format!("end{tag_name}");
    let tag = name_index(tag_name);
    if let (Some(cache), Some(tag)) = (cache.as_deref_mut(), tag) {
        if cache.absent_from(source, tag, &end_name, body_start) {
            return None;
        }
        if let Some(&known) = cache.known.get(&(tag, body_start)) {
            return known;
        }
    }
    let nests = rule.same_name == SameName::Nests;
    // Where the bodies of the nested openers still open start, and, under a rule
    // that doesn't nest, each position this search reads a token from.
    let mut open: Vec<usize> = Vec::new();
    let mut read: Vec<usize> = Vec::new();
    let caching = cache.is_some() && tag.is_some();
    let finish = |cache: Option<&mut RawBodyEndCache>, open: &[usize], read: &[usize], end| {
        if let (Some(cache), Some(tag)) = (cache, tag) {
            for &start in open {
                cache.known.insert((tag, start), None);
            }
            for &start in read {
                cache.known.insert((tag, start), end);
            }
        }
        end
    };
    let bytes = source.as_bytes();
    let mut token_start = body_start;
    let mut depth = 1u32;
    while token_start < bytes.len() {
        if caching && !nests {
            if let Some(&earlier) =
                cache.as_deref().and_then(|c| c.known.get(&(tag.unwrap_or(0), token_start)))
            {
                return finish(cache, &open, &read, earlier);
            }
            read.push(token_start);
        }
        // Text cannot name a tag: advance straight to the next Liquid token.
        if !(bytes[token_start] == b'{'
            && matches!(bytes.get(token_start + 1), Some(b'%') | Some(b'{')))
        {
            match next_liquid_open(bytes, token_start) {
                Some(next) => token_start = next,
                None => return finish(cache, &open, &read, None),
            }
        }
        let token_end = if bytes[token_start + 1] == b'%' {
            tag_token_end(bytes, token_start + 2)
        } else {
            variable_token_end(bytes, token_start + 2)
        };
        let token = &source[token_start..token_end];

        match token_tag(token, rule.delimiter) {
            Some((name, index)) if name == end_name => {
                let tag_start = token_start + index;
                let end = RawBodyEnd {
                    tag_start: tag_start as u32,
                    tag_end: token_end as u32,
                    ws_start: if bytes.get(tag_start + 2) == Some(&b'-') { "-" } else { "" },
                    ws_end: if token.as_bytes()[token.len() - 3] == b'-' { "-" } else { "" },
                };
                depth -= 1;
                if depth == 0 {
                    return finish(cache, &open, &read, Some(end));
                }
                if let (Some(nested), Some(c), Some(tag)) = (open.pop(), cache.as_deref_mut(), tag) {
                    c.known.insert((tag, nested), Some(end));
                }
            }
            Some((name, _)) if name == tag_name => match rule.same_name {
                SameName::Rejected => return finish(cache, &open, &read, None),
                SameName::Nests => {
                    depth += 1;
                    if caching {
                        open.push(token_end);
                    }
                }
                SameName::Ignored => {}
            },
            Some((name, _)) if rule.opaque.contains(&name) => {
                if let Some(inner_rule) = rule_for(name) {
                    match find_raw_body_end(source, token_end, name, inner_rule, cache.as_deref_mut()) {
                        Some(inner) => {
                            token_start = inner.tag_end as usize;
                            continue;
                        }
                        None => return finish(cache, &open, &read, None),
                    }
                }
            }
            _ => {}
        }
        token_start = token_end;
    }
    finish(cache, &open, &read, None)
}

/// `lineTagName`: `BlockBody::LiquidTagToken` indents only with ASCII whitespace,
/// so a line indented with a character such as U+00A0 has no name.
fn line_tag_name<'s>(source: &str, line: &LiquidLine<'s>) -> Option<&'s str> {
    let name_offset = line.name_offset as usize;
    let b = source.as_bytes();
    // `source.lastIndexOf('\n', nameOffset - 1) + 1`.
    let line_start = memchr::memrchr(b'\n', &b[..name_offset]).map_or(0, |i| i + 1);
    let indent = &b[line_start..name_offset];
    if !indent.iter().all(|&c| matches!(c, b' ' | b'\t' | 0x0B | 0x0C | b'\r')) {
        return None;
    }
    let t = line.tag_name.as_bytes();
    let end = if t.first() == Some(&b'#') { 1 } else { word_end(t, 0) };
    (end > 0).then(|| &line.tag_name[..end])
}

/// `findRawLineBodyEnd`: the index of the line that ends the body of `tag_name`
/// starting at `lines[from]` inside `{% liquid %}`.
pub(crate) fn find_raw_line_body_end(
    source: &str,
    lines: &[LiquidLine<'_>],
    from: usize,
    tag_name: &str,
    rule: &RawBodyRule,
) -> Option<usize> {
    if !rule.closes_in_liquid_tag {
        return None;
    }
    let mut depth = 1u32;
    let mut index = from;
    while index < lines.len() {
        let name = line_tag_name(source, &lines[index]);
        match name {
            Some(n) if n.strip_prefix("end") == Some(tag_name) => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            Some(n) if n == tag_name => match rule.same_name {
                SameName::Rejected => return None,
                SameName::Nests => depth += 1,
                SameName::Ignored => {}
            },
            Some(n) if rule.opaque.contains(&n) => {
                if let Some(inner_rule) = rule_for(n) {
                    index = find_raw_line_body_end(source, lines, index + 1, n, inner_rule)?;
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end(source: &str, name: &str) -> Option<(u32, u32)> {
        let open = source.find("%}").unwrap() + 2;
        find_raw_body_end(source, open, name, rule_for(name).unwrap(), None)
            .map(|e| (e.tag_start, e.tag_end))
    }

    #[test]
    fn raw_closes_at_the_last_tag_of_its_end_token() {
        let s = "{% raw %}a{% x {% endraw %}b";
        assert_eq!(end(s, "raw"), Some((15, 27)));
        assert_eq!(end("{% raw %}{% endraw x %}", "raw"), Some((9, 23)));
    }

    #[test]
    fn comment_nests_and_skips_raw() {
        let s = "{% comment %}{% comment %}{% endcomment %}{% raw %}{% endcomment %}{% endraw %}{% endcomment %}";
        assert_eq!(end(s, "comment").map(|e| e.0 as usize), Some(s.len() - 16));
        assert_eq!(end("{% doc %}{% doc %}{% enddoc %}", "doc"), None);
    }

    #[test]
    fn token_names() {
        assert_eq!(liquid_tag_token_name("{%- comment -%}"), Some("comment"));
        assert_eq!(liquid_tag_token_name("{%#x%}"), Some("#"));
        assert_eq!(liquid_tag_token_name("{%-%}"), None);
        assert_eq!(liquid_tag_token_name("{%%}"), None);
    }
}
