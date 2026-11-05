//! Native port of what Prettier does for `parser: 'json'`.
//!
//! In Prettier 3.9.10 only `json-stringify` uses the `estree-json` printer; `json` goes
//! through the JavaScript `estree` printer. So this ports the object, array, property,
//! literal and comment paths of that printer, restricted to the node types the JSON
//! parser produces. For `json`, `quoteProps` has no effect (keys are always quoted when
//! they are identifiers or simple numbers) and `trailingComma` is forced to `none`.

pub mod comments;
pub mod parser;
pub mod skip;

use crate::concat;
use crate::doc::utils::replace_end_of_line;
use crate::doc::{group, group_with, indent, line_suffix, Doc};
use crate::options::{ObjectWrap, Options};
use crate::printer::string::{js_trim, js_trim_end, js_trim_start};
use crate::printer::Ctx;
use parser::*;
use skip::*;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};

struct Printer<'a, 'c> {
    ast: &'c Ast<'a>,
    options: &'c Options,
    ctx: &'c Ctx<'a>,
    printed: RefCell<Vec<bool>>,
    ignored_printed: RefCell<Vec<bool>>,
    failed: Cell<bool>,
}

/// `printNumber`.
fn print_number(raw: &str) -> Cow<'_, str> {
    if raw.len() == 1 {
        return Cow::Borrowed(raw);
    }
    let mut s = raw.to_ascii_lowercase();

    let is_float_char = |b: u8| b.is_ascii_digit() || b == b'.';
    // ^([+-]?[\d.]+e)(?:\+|(-))?0*(?=\d) -> $1$2
    {
        let bytes = s.as_bytes();
        let mut i = 0;
        if matches!(bytes.first(), Some(b'+' | b'-')) {
            i += 1;
        }
        let mantissa_start = i;
        while i < bytes.len() && is_float_char(bytes[i]) {
            i += 1;
        }
        if i > mantissa_start && bytes.get(i) == Some(&b'e') {
            let after_e = i + 1;
            let mut j = after_e;
            let mut minus = false;
            if bytes.get(j) == Some(&b'+') {
                j += 1;
            } else if bytes.get(j) == Some(&b'-') {
                minus = true;
                j += 1;
            }
            let mut k = j;
            while bytes.get(k) == Some(&b'0') {
                k += 1;
            }
            // `0*` backtracks so that a digit follows.
            let k = if bytes.get(k).is_some_and(u8::is_ascii_digit) {
                Some(k)
            } else if k > j {
                Some(k - 1)
            } else {
                None
            };
            if let Some(k) = k {
                let mut out = s[..after_e].to_string();
                if minus {
                    out.push('-');
                }
                out.push_str(&s[k..]);
                s = out;
            }
        }
    }
    // ^([+-]?[\d.]+)e[+-]?0+$ -> $1
    {
        let bytes = s.as_bytes();
        let mut i = 0;
        if matches!(bytes.first(), Some(b'+' | b'-')) {
            i += 1;
        }
        let mantissa_start = i;
        while i < bytes.len() && is_float_char(bytes[i]) {
            i += 1;
        }
        if i > mantissa_start && bytes.get(i) == Some(&b'e') {
            let mut j = i + 1;
            if matches!(bytes.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            let zeros = j;
            while bytes.get(j) == Some(&b'0') {
                j += 1;
            }
            if j > zeros && j == bytes.len() {
                s.truncate(i);
            }
        }
    }
    // ^([+-])?\. -> $10.
    {
        let (sign, rest) = match s.as_bytes().first() {
            Some(b'+' | b'-') => s.split_at(1),
            _ => ("", s.as_str()),
        };
        if rest.starts_with('.') {
            s = format!("{sign}0{rest}");
        }
    }
    // (\.\d+?)0+(?=e|$) -> $1, first match only
    {
        let bytes = s.as_bytes();
        let mut replaced = None;
        let mut start = 0;
        while let Some(off) = s[start..].find('.') {
            let dot = start + off;
            let mut end = dot + 1;
            while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            if end == bytes.len() || bytes[end] == b'e' {
                // Trailing zeros, keeping at least one digit after the dot.
                let mut keep = end;
                while keep > dot + 2 && bytes[keep - 1] == b'0' {
                    keep -= 1;
                }
                if keep < end {
                    replaced = Some((keep, end));
                    break;
                }
            }
            start = dot + 1;
        }
        if let Some((keep, end)) = replaced {
            s.replace_range(keep..end, "");
        }
    }
    // \.(?=e|$) -> "", first match only
    {
        let bytes = s.as_bytes();
        let mut found = None;
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'.' && (i + 1 == bytes.len() || bytes[i + 1] == b'e') {
                found = Some(i);
                break;
            }
        }
        if let Some(i) = found {
            s.remove(i);
        }
    }
    if s == raw {
        Cow::Borrowed(raw)
    } else {
        Cow::Owned(s)
    }
}

/// `String(Number(raw))` for a decimal literal.
fn js_number_string(v: f64) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    if !v.is_finite() {
        return "Infinity".to_string();
    }
    let sci = format!("{v:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let k = digits.len() as i32;
    let n = exp + 1;
    if k <= n && n <= 21 {
        return format!("{digits}{}", "0".repeat((n - k) as usize));
    }
    if 0 < n && n <= 21 {
        let (a, b) = digits.split_at(n as usize);
        return format!("{a}.{b}");
    }
    if -6 < n && n <= 0 {
        return format!("0.{}{digits}", "0".repeat((-n) as usize));
    }
    let e = n - 1;
    let sign = if e >= 0 { "+" } else { "-" };
    let e = e.abs();
    if k == 1 {
        return format!("{digits}e{sign}{e}");
    }
    let (a, b) = digits.split_at(1);
    format!("{a}.{b}e{sign}{e}")
}

/// `/^(?:\d+|\d+\.\d+)$/`.
fn is_simple_number(s: &str) -> bool {
    let (a, b) = match s.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    !a.is_empty()
        && a.bytes().all(|c| c.is_ascii_digit())
        && b.is_none_or(|b| !b.is_empty() && b.bytes().all(|c| c.is_ascii_digit()))
}

/// `JSON.stringify(name)`.
fn json_stringify(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `printString` with `"` forced, as for `parser: 'json'`.
fn print_string(raw: &str) -> Cow<'_, str> {
    if raw.starts_with('"') {
        return Cow::Borrowed(raw);
    }
    let body = &raw[1..raw.len() - 1];
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('"');
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.clone().next() {
                Some('\'') => {
                    chars.next();
                    out.push('\'');
                }
                Some('"') => {
                    chars.next();
                    out.push_str("\\\"");
                }
                Some('\\') => {
                    chars.next();
                    out.push_str("\\\\");
                }
                _ => out.push('\\'),
            },
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out.push('"');
    Cow::Owned(out)
}

/// `isIndentableBlockComment`'s `getCommentLines`: empty when not indentable.
fn indentable_block_comment_lines(value: &str) -> Vec<String> {
    if !value.contains('\n') {
        return Vec::new();
    }
    let full = format!("*{value}*");
    let mut out = Vec::new();
    for l in full.split('\n') {
        let l = js_trim_start(l);
        if !l.starts_with('*') {
            return Vec::new();
        }
        out.push(l.to_string());
    }
    out
}

impl<'a> Printer<'a, '_> {
    #[inline]
    fn node(&self, id: Id) -> &JsonNode<'a> {
        &self.ast.nodes[id as usize]
    }

    #[inline]
    fn text(&self) -> &'a str {
        self.ast.text
    }

    fn comment(&self, c: u32) -> &Comment<'a> {
        &self.ast.comments[c as usize]
    }

    /// `printComment`.
    fn print_comment(&self, c: u32) -> Doc<'a> {
        self.printed.borrow_mut()[c as usize] = true;
        let comment = self.comment(c);
        let text = self.text();
        if !comment.block {
            return js_trim_end(&text[comment.start..comment.end]).into();
        }
        let lines = indentable_block_comment_lines(comment.value);
        if !lines.is_empty() {
            let is_jsdoc = {
                let mut v = comment.value.chars();
                v.next() == Some('*') && v.next() != Some('*')
            };
            let last = lines.len() - 1;
            let mut parts: Vec<Doc<'a>> = vec!["/".into()];
            for (i, n) in lines.iter().enumerate() {
                if i == 0 {
                    parts.push(js_trim_end(n).to_string().into());
                    parts.push(Doc::Hardline);
                } else if i == last {
                    parts.push(" ".into());
                    parts.push(n.clone().into());
                } else {
                    let trimmed = js_trim_end(n);
                    parts.push(" ".into());
                    parts.push(trimmed.to_string().into());
                    if is_jsdoc && trimmed != "*" && n.ends_with("  ") {
                        parts.push("  ".into());
                        parts.push(Doc::RootedLiteralline);
                    } else {
                        parts.push(Doc::Hardline);
                    }
                }
            }
            parts.push("/".into());
            return Doc::Concat(parts);
        }
        concat!["/*", replace_end_of_line(comment.value), "*/"]
    }

    /// `printLeadingComment`.
    fn print_leading_comment(&self, c: u32) -> Doc<'a> {
        let printed = self.print_comment(c);
        let comment = self.comment(c);
        let text = self.text();
        let mut parts = vec![printed];
        if comment.block {
            let sep = if has_newline(text, comment.end) {
                if has_newline_backwards(text, comment.start) {
                    Doc::Hardline
                } else {
                    Doc::Line
                }
            } else {
                " ".into()
            };
            parts.push(sep);
        } else {
            parts.push(Doc::Hardline);
        }
        let n = skip_newline(text, skip_spaces(text, Some(comment.end), false), false);
        if n.is_some_and(|n| has_newline(text, n)) {
            parts.push(Doc::Hardline);
        }
        Doc::Concat(parts)
    }

    /// `printTrailingComment`, returning `{ doc, isBlock, hasLineSuffix }`.
    fn print_trailing_comment(&self, c: u32, prev: Option<(bool, bool)>) -> (Doc<'a>, bool, bool) {
        let printed = self.print_comment(c);
        let comment = self.comment(c);
        let text = self.text();
        let is_block = comment.block;
        let (prev_is_block, prev_has_suffix) = prev.unwrap_or((false, false));
        if (prev_has_suffix && !prev_is_block) || has_newline_backwards(text, comment.start) {
            let blank = is_previous_line_empty(text, comment.start);
            let doc = line_suffix(concat![
                Doc::Hardline,
                if blank { Doc::Hardline } else { Doc::Nil },
                printed,
            ]);
            return (doc, is_block, true);
        }
        if !is_block || prev_has_suffix {
            return (concat![line_suffix(concat![" ", printed]), Doc::BreakParent], is_block, true);
        }
        (concat![" ", printed], is_block, false)
    }

    /// `printComments`.
    fn print_comments(&self, id: Id, doc: Doc<'a>) -> Doc<'a> {
        let node = self.node(id);
        if node.comments.is_empty() {
            return doc;
        }
        let mut leading = Vec::new();
        for &c in &node.comments {
            if self.comment(c).leading && !self.ignored_printed.borrow()[c as usize] {
                leading.push(self.print_leading_comment(c));
            }
        }
        let mut trailing = Vec::new();
        let mut prev = None;
        for &c in &node.comments {
            if self.comment(c).trailing {
                let skip = self.ignored_printed.borrow()[c as usize];
                let (d, is_block, has_suffix) = self.print_trailing_comment(c, prev);
                prev = Some((is_block, has_suffix));
                if !skip {
                    trailing.push(d);
                }
            }
        }
        if leading.is_empty() && trailing.is_empty() {
            return doc;
        }
        concat![Doc::Concat(leading), doc, Doc::Concat(trailing)]
    }

    /// `printDanglingComments`, without `indent`.
    fn print_dangling(&self, id: Id) -> Doc<'a> {
        let node = self.node(id);
        let mut parts = Vec::new();
        for &c in &node.comments {
            let comment = self.comment(c);
            if !comment.leading && !comment.trailing {
                if !parts.is_empty() {
                    parts.push(Doc::Hardline);
                }
                parts.push(self.print_comment(c));
            }
        }
        if parts.is_empty() {
            Doc::Nil
        } else {
            Doc::Concat(parts)
        }
    }

    fn has_dangling(&self, id: Id, line_only: bool) -> bool {
        self.node(id).comments.iter().any(|&c| {
            let comment = self.comment(c);
            !comment.leading && !comment.trailing && (!line_only || !comment.block)
        })
    }

    /// `printDanglingCommentsInList`.
    fn print_dangling_in_list(&self, id: Id) -> Doc<'a> {
        if !self.has_dangling(id, false) {
            return Doc::Nil;
        }
        concat![
            indent(concat![Doc::Softline, self.print_dangling(id)]),
            if self.has_dangling(id, true) { Doc::Hardline } else { Doc::Softline },
        ]
    }

    /// `hasIgnoreComment`.
    fn has_prettier_ignore(&self, id: Id) -> bool {
        self.node(id).comments.iter().any(|&c| js_trim(self.comment(c).value) == "prettier-ignore")
    }

    /// `mainPrint` + `callPluginPrintFunction`.
    fn print(&self, id: Id) -> Doc<'a> {
        let doc = if self.has_prettier_ignore(id) {
            let node = self.node(id);
            let (start, end) = (node.start, node.end);
            for (i, c) in self.ast.comments.iter().enumerate() {
                if c.start >= start && c.end <= end {
                    self.ignored_printed.borrow_mut()[i] = true;
                }
            }
            Doc::from(&self.text()[start..end])
        } else {
            self.print_inner(id)
        };
        self.print_comments(id, doc)
    }

    fn print_inner(&self, id: Id) -> Doc<'a> {
        match &self.node(id).kind {
            Kind::Root { node } => concat![self.print_dangling(id), self.print(*node)],
            Kind::Null => "null".into(),
            Kind::Boolean(v) => (*v).into(),
            Kind::Str(raw) => match print_string(raw) {
                Cow::Borrowed(s) => replace_end_of_line(s),
                Cow::Owned(s) => {
                    if s.contains('\n') {
                        let mut parts = Vec::new();
                        for (i, l) in s.split('\n').enumerate() {
                            if i > 0 {
                                parts.push(Doc::Literalline);
                            }
                            parts.push(l.to_string().into());
                        }
                        Doc::Concat(parts)
                    } else {
                        s.into()
                    }
                }
            },
            Kind::Numeric(raw) => print_number(raw).into(),
            Kind::Identifier(name) => match name {
                Cow::Borrowed(s) => (*s).into(),
                Cow::Owned(s) => s.clone().into(),
            },
            Kind::Template { raw } => {
                concat![Doc::LineSuffixBoundary, "`", replace_end_of_line(raw), "`"]
            }
            Kind::Unary { operator, argument } => {
                let printed = self.print(*argument);
                if self.node(*argument).comments.is_empty() {
                    concat![*operator, printed]
                } else {
                    concat![
                        *operator,
                        group(concat![
                            "(",
                            indent(concat![Doc::Softline, printed]),
                            Doc::Softline,
                            ")",
                        ]),
                    ]
                }
            }
            Kind::Object { properties } => self.print_object(id, properties),
            Kind::Array { elements } => self.print_array(id, elements),
            Kind::Property { key, value } => self.print_property(*key, *value),
        }
    }

    fn print_property_key(&self, key: Id) -> Doc<'a> {
        let quoted = match &self.node(key).kind {
            Kind::Identifier(name) => Some(json_stringify(name)),
            Kind::Numeric(raw) => {
                let n = print_number(raw);
                if is_simple_number(&n) {
                    let value: f64 = n.parse().unwrap_or(f64::NAN);
                    let s = js_number_string(value);
                    (s == *n).then(|| json_stringify(&s))
                } else {
                    None
                }
            }
            _ => None,
        };
        match quoted {
            Some(s) => self.print_comments(key, s.into()),
            None => self.print(key),
        }
    }

    fn print_property(&self, key: Id, value: Id) -> Doc<'a> {
        let printed_key = self.print_property_key(key);
        let text = self.text();
        let break_after_operator = self.node(value).comments.iter().any(|&c| {
            let comment = self.comment(c);
            comment.leading
                && (has_newline(text, comment.end)
                    || (comment.block && !indentable_block_comment_lines(comment.value).is_empty()))
        });
        let printed_value = self.print(value);
        if break_after_operator {
            group(concat![
                group(printed_key),
                ":",
                group(indent(concat![Doc::Line, printed_value])),
            ])
        } else {
            group(concat![group(printed_key), ":", " ", printed_value])
        }
    }

    fn print_object(&self, id: Id, properties: &[Id]) -> Doc<'a> {
        let text = self.text();
        if properties.is_empty() {
            return group(concat!["{", self.print_dangling_in_list(id), "}"]);
        }
        let should_break = self.options.object_wrap == ObjectWrap::Preserve
            && has_newline_in_range(text, self.node(id).start, self.node(properties[0]).start);
        let bracket_line = || {
            if self.options.bracket_spacing {
                Doc::Line
            } else {
                Doc::Softline
            }
        };
        let mut parts = vec![bracket_line()];
        let mut prev: Option<Id> = None;
        for &p in properties {
            if let Some(q) = prev {
                parts.push(",".into());
                parts.push(Doc::Line);
                if is_next_line_empty(text, self.node(q).end) {
                    parts.push(Doc::Hardline);
                }
            }
            parts.push(self.print(p));
            prev = Some(p);
        }
        group_with(concat!["{", indent(parts), bracket_line(), "}"], 0, should_break)
    }

    /// `isLineAfterElementEmpty`.
    fn is_line_after_element_empty(&self, id: Id) -> bool {
        let text = self.text();
        let bytes = text.as_bytes();
        let node = self.node(id);
        let mut r = node.end;
        if r == node.start {
            return false;
        }
        while r < bytes.len() && bytes[r] != b',' {
            let n = skip_trailing_comment(text, Some(r + 1));
            r = skip_inline_comment(text, n).unwrap_or(bytes.len());
        }
        is_next_line_empty(text, r)
    }

    fn is_numeric(&self, id: Id) -> bool {
        matches!(self.node(id).kind, Kind::Numeric(_))
    }

    /// `isConciselyPrintedArray`.
    fn is_concisely_printed_array(&self, elements: &[Option<Id>]) -> bool {
        let text = self.text();
        !elements.is_empty()
            && elements.iter().all(|e| {
                let Some(e) = *e else {
                    return false;
                };
                let numeric = self.is_numeric(e)
                    || matches!(self.node(e).kind, Kind::Unary { argument, .. }
                        if self.is_numeric(argument) && self.node(argument).comments.is_empty());
                numeric
                    && !self.node(e).comments.iter().any(|&c| {
                        let comment = self.comment(c);
                        comment.trailing
                            && !comment.block
                            && !has_newline_backwards(text, comment.start)
                    })
            })
    }

    /// `len > 1` of `node.elements` / `node.properties`, for the matrix check.
    fn collection_len(&self, id: Id) -> Option<(u8, usize)> {
        match &self.node(id).kind {
            Kind::Array { elements } => Some((0, elements.len())),
            Kind::Object { properties } => Some((1, properties.len())),
            _ => None,
        }
    }

    fn print_array(&self, id: Id, elements: &[Option<Id>]) -> Doc<'a> {
        if elements.is_empty() {
            return group(concat!["[", self.print_dangling_in_list(id), "]"]);
        }
        let last_is_hole = elements[elements.len() - 1].is_none();
        let group_id = self.ctx.symbol();
        let should_break = (elements.len() > 1
            && elements.iter().enumerate().all(|(i, e)| {
                let Some(e) = *e else {
                    return false;
                };
                let Some((ty, len)) = self.collection_len(e) else {
                    return false;
                };
                if let Some(Some(n)) = elements.get(i + 1) {
                    if self.collection_len(*n).map(|c| c.0) != Some(ty) {
                        return false;
                    }
                }
                len > 1
            }))
            || self.has_dangling(id, true);

        // `shouldPrintComma` is false: `trailingComma` is `none` for `parser: 'json'`.
        let trailing_comma = if last_is_hole { Doc::Str(",") } else { Doc::Nil };

        let items = if self.is_concisely_printed_array(elements) {
            let mut parts = Vec::with_capacity(elements.len() * 2);
            for (i, e) in elements.iter().enumerate() {
                let e = e.unwrap_or(0);
                let is_last = i + 1 == elements.len();
                parts.push(concat![
                    self.print(e),
                    if is_last { Doc::Nil } else { Doc::Str(",") },
                ]);
                if !is_last {
                    let next = elements[i + 1].unwrap_or(0);
                    parts.push(if self.is_line_after_element_empty(e) {
                        concat![Doc::Hardline, Doc::Hardline]
                    } else if self.node(next).comments.iter().any(|&c| {
                        let comment = self.comment(c);
                        comment.leading && !comment.block
                    }) {
                        Doc::Hardline
                    } else {
                        Doc::Line
                    });
                }
            }
            Doc::Fill(parts)
        } else {
            let mut parts = Vec::with_capacity(elements.len() * 2 + 1);
            for (i, e) in elements.iter().enumerate() {
                parts.push(match e {
                    Some(e) => group(self.print(*e)),
                    None => Doc::Nil,
                });
                if i + 1 != elements.len() {
                    parts.push(concat![
                        ",",
                        Doc::Line,
                        match e {
                            Some(e) if self.is_line_after_element_empty(*e) => Doc::Softline,
                            _ => Doc::Nil,
                        },
                    ]);
                }
            }
            parts.push(trailing_comma);
            Doc::Concat(parts)
        };

        group_with(
            concat![
                "[",
                indent(concat![Doc::Softline, items, self.print_dangling(id)]),
                Doc::Softline,
                "]",
            ],
            group_id,
            should_break,
        )
    }
}

/// `textToDoc(text, { parser: 'json' })`, including `stripTrailingHardline`. `None` when
/// Prettier would throw: a parse failure, or a comment that ends up unprinted.
pub fn text_to_doc<'a>(text: &'a str, ctx: &Ctx<'a>) -> Option<Doc<'a>> {
    let mut ast = parser::parse(text).ok()?;
    comments::attach_comments(&mut ast);
    let count = ast.comments.len();
    let printer = Printer {
        ast: &ast,
        options: ctx.options,
        ctx,
        printed: RefCell::new(vec![false; count]),
        ignored_printed: RefCell::new(vec![false; count]),
        failed: Cell::new(false),
    };
    let doc = printer.print(ast.root);
    let printed = printer.printed.borrow();
    let ignored = printer.ignored_printed.borrow();
    if printer.failed.get() || (0..count).any(|i| !printed[i] && !ignored[i]) {
        return None;
    }
    Some(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        for (a, b) in [
            ("1.0", "1.0"),
            ("1.50", "1.5"),
            ("1.00", "1.0"),
            (".5", "0.5"),
            ("5.", "5"),
            ("1E3", "1e3"),
            ("1e+3", "1e3"),
            ("1e-3", "1e-3"),
            ("1e00", "1"),
            ("0XAB", "0xab"),
            ("1_000", "1_000"),
            ("5.e3", "5e3"),
        ] {
            assert_eq!(print_number(a), b, "{a}");
        }
    }

    #[test]
    fn js_numbers() {
        assert_eq!(js_number_string(100.0), "100");
        assert_eq!(js_number_string(0.1), "0.1");
        assert_eq!(js_number_string(1.5), "1.5");
        assert_eq!(js_number_string(1e21), "1e+21");
        assert_eq!(js_number_string(0.0000001), "1e-7");
    }

    #[test]
    fn strings() {
        assert_eq!(print_string("'a'"), "\"a\"");
        assert_eq!(print_string("'it\\'s'"), "\"it's\"");
        assert_eq!(print_string("'q\"x'"), "\"q\\\"x\"");
        assert_eq!(print_string("\"a'b\""), "\"a'b\"");
    }

    #[test]
    fn parser_rejects() {
        for bad in ["{a}", "(1)", "1n", "{[a]:1}", "--1", "a", "/* only */", "1 2", "{\"__proto__\":1,__proto__:2}"] {
            assert!(parser::parse(bad).is_err(), "{bad}");
        }
        for good in ["[,]", "- 1", "{a:1,}", "`x`", "NaN", "0x10", "'\\1'", "089.5", "<!-- h\n1"] {
            assert!(parser::parse(good).is_ok(), "{good}");
        }
    }
}
