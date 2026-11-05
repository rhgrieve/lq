//! A port of the `yaml` npm package (2.9.1) with its default options: the lexer,
//! CST parser and composer, core schema, `parse` and `stringify`.
//!
//! All offsets are UTF-8 bytes here. Error positions are converted to UTF-16 line
//! and column when the message is prettified, so messages match upstream.
//!
//! Deviations from upstream:
//! - `%YAML 1.1` does not switch to the YAML 1.1 schema.
//! - Warnings (unresolved tags, `Alias ending in :`) are not printed.
//! - An alias inside its own anchored node sees an empty container rather than
//!   the circular reference upstream produces.
//! - A collection used as a map key is stringified from its resolved value, so
//!   quoting styles from the source are not preserved in the key string.
//! - A `\uD800`-style lone surrogate escape becomes U+FFFD.
//! - `stringify` works on plain values: shared references cannot exist, so it
//!   never emits the `&a1`/`*a1` that upstream's `aliasDuplicateObjects` can.

use lq_json::js::{self, utf16_len};
use lq_json::regex::JsRegex;
use lq_json::value::{Object, Value};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct YamlError {
    /// `error.name`: `YAMLParseError` or `ReferenceError`.
    pub name: String,
    /// `error.message`, including the prettified code frame.
    pub message: String,
    /// `error.code` for a `YAMLParseError`.
    pub code: Option<&'static str>,
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

const DOCUMENT: &str = "\u{2}";
const FLOW_END: &str = "\u{18}";
const SCALAR: &str = "\u{1f}";
const BOM: &str = "\u{FEFF}";

fn is_empty_ch(c: Option<u8>) -> bool {
    matches!(c, None | Some(b' ' | b'\n' | b'\r' | b'\t'))
}

fn is_flow_indicator(c: Option<u8>) -> bool {
    matches!(c, Some(b',' | b'[' | b']' | b'{' | b'}'))
}

fn is_not_anchor_char(c: Option<u8>) -> bool {
    matches!(c, None | Some(b' ' | b',' | b'[' | b']' | b'{' | b'}' | b'\n' | b'\r' | b'\t'))
}

fn is_tag_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"-#;/?:@&=+$_.!~*'()".contains(&c)
}

fn index_of(b: &[u8], c: u8, from: usize) -> Option<usize> {
    if from >= b.len() {
        return None;
    }
    b[from..].iter().position(|x| *x == c).map(|p| p + from)
}

#[derive(Clone, Copy)]
enum LexState {
    Stream,
    BlockStart,
    Doc,
    Flow,
}

struct Lexer<'a> {
    src: &'a str,
    b: &'a [u8],
    pos: usize,
    block_scalar_indent: i64,
    block_scalar_keep: bool,
    flow_key: bool,
    flow_level: i64,
    indent_next: i64,
    indent_value: i64,
    out: Vec<&'a str>,
}

impl<'a> Lexer<'a> {
    fn lex(src: &'a str) -> Vec<&'a str> {
        let mut l = Lexer {
            src,
            b: src.as_bytes(),
            pos: 0,
            block_scalar_indent: -1,
            block_scalar_keep: false,
            flow_key: false,
            flow_level: 0,
            indent_next: 0,
            indent_value: 0,
            out: Vec::new(),
        };
        let mut next = LexState::Stream;
        while l.pos < l.b.len() {
            next = match next {
                LexState::Stream => l.parse_stream(),
                LexState::BlockStart => l.parse_block_start(),
                LexState::Doc => l.parse_document(),
                LexState::Flow => l.parse_flow(),
            };
        }
        l.out
    }

    fn at(&self, i: i64) -> Option<u8> {
        if i < 0 {
            return None;
        }
        self.b.get(i as usize).copied()
    }

    fn char_at(&self, n: usize) -> Option<u8> {
        self.b.get(self.pos + n).copied()
    }

    fn at_line_end(&self) -> bool {
        let mut i = self.pos;
        while matches!(self.b.get(i), Some(b' ' | b'\t')) {
            i += 1;
        }
        match self.b.get(i) {
            None | Some(b'#' | b'\n') => true,
            Some(b'\r') => self.b.get(i + 1) == Some(&b'\n'),
            _ => false,
        }
    }

    fn continue_scalar(&self, offset: usize) -> i64 {
        let mut ch = self.b.get(offset).copied();
        if self.indent_next > 0 {
            let mut indent = 0usize;
            while ch == Some(b' ') {
                indent += 1;
                ch = self.b.get(indent + offset).copied();
            }
            if ch == Some(b'\r') && self.b.get(indent + offset + 1) == Some(&b'\n') {
                return (offset + indent + 1) as i64;
            }
            return if ch == Some(b'\n') || indent as i64 >= self.indent_next {
                (offset + indent) as i64
            } else {
                -1
            };
        }
        if ch == Some(b'-') || ch == Some(b'.') {
            let dt = &self.b[offset..(offset + 3).min(self.b.len())];
            if (dt == b"---" || dt == b"...") && is_empty_ch(self.b.get(offset + 3).copied()) {
                return -1;
            }
        }
        offset as i64
    }

    fn get_line(&self) -> &'a str {
        let src: &'a str = self.src;
        match index_of(self.b, b'\n', self.pos) {
            None => &src[self.pos..],
            Some(mut end) => {
                if end > 0 && self.b[end - 1] == b'\r' {
                    end -= 1;
                }
                if end < self.pos {
                    end = self.pos;
                }
                &src[self.pos..end]
            }
        }
    }

    fn push_count(&mut self, n: usize) -> usize {
        if n > 0 {
            let src: &'a str = self.src;
            let end = (self.pos + n).min(self.b.len());
            self.out.push(&src[self.pos..end]);
            let got = end - self.pos;
            self.pos = end;
            return got;
        }
        0
    }

    fn push_to_index(&mut self, i: usize, allow_empty: bool) -> usize {
        let src: &'a str = self.src;
        let i = i.min(self.b.len());
        if i > self.pos {
            self.out.push(&src[self.pos..i]);
            let n = i - self.pos;
            self.pos = i;
            n
        } else {
            if allow_empty {
                self.out.push("");
            }
            0
        }
    }

    fn push_newline(&mut self) -> usize {
        match self.b.get(self.pos) {
            Some(b'\n') => self.push_count(1),
            Some(b'\r') if self.char_at(1) == Some(b'\n') => self.push_count(2),
            _ => 0,
        }
    }

    fn push_spaces(&mut self, allow_tabs: bool) -> usize {
        let mut i = self.pos;
        while let Some(&c) = self.b.get(i) {
            if c == b' ' || (allow_tabs && c == b'\t') {
                i += 1;
            } else {
                break;
            }
        }
        let n = i - self.pos;
        if n > 0 {
            let src: &'a str = self.src;
            self.out.push(&src[self.pos..i]);
            self.pos = i;
        }
        n
    }

    fn push_until(&mut self, test: impl Fn(Option<u8>) -> bool) -> usize {
        let mut i = self.pos;
        while !test(self.b.get(i).copied()) {
            i += 1;
        }
        self.push_to_index(i, false)
    }

    fn push_tag(&mut self) -> usize {
        if self.char_at(1) == Some(b'<') {
            let mut i = self.pos + 2;
            let mut ch = self.b.get(i).copied();
            while !is_empty_ch(ch) && ch != Some(b'>') {
                i += 1;
                ch = self.b.get(i).copied();
            }
            self.push_to_index(if ch == Some(b'>') { i + 1 } else { i }, false)
        } else {
            let mut i = self.pos + 1;
            while let Some(&c) = self.b.get(i) {
                if is_tag_char(c) {
                    i += 1;
                } else if c == b'%'
                    && self.b.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                    && self.b.get(i + 2).is_some_and(u8::is_ascii_hexdigit)
                {
                    i += 3;
                } else {
                    break;
                }
            }
            self.push_to_index(i, false)
        }
    }

    fn push_indicators(&mut self) -> usize {
        let mut n = 0;
        loop {
            match self.char_at(0) {
                Some(b'!') => {
                    n += self.push_tag();
                    n += self.push_spaces(true);
                    continue;
                }
                Some(b'&') => {
                    n += self.push_until(is_not_anchor_char);
                    n += self.push_spaces(true);
                    continue;
                }
                Some(b'-' | b'?' | b':') => {
                    let in_flow = self.flow_level > 0;
                    let ch1 = self.char_at(1);
                    if is_empty_ch(ch1) || (in_flow && is_flow_indicator(ch1)) {
                        if !in_flow {
                            self.indent_next = self.indent_value + 1;
                        } else if self.flow_key {
                            self.flow_key = false;
                        }
                        n += self.push_count(1);
                        n += self.push_spaces(true);
                        continue;
                    }
                }
                _ => {}
            }
            break;
        }
        n
    }

    fn parse_stream(&mut self) -> LexState {
        let mut line = self.get_line();
        if line.starts_with(BOM) {
            self.push_count(BOM.len());
            line = &line[BOM.len()..];
        }
        if line.starts_with('%') {
            let lb = line.as_bytes();
            let mut dir_end = lb.len() as i64;
            let mut cs = index_of(lb, b'#', 0);
            while let Some(c) = cs {
                if c > 0 && matches!(lb[c - 1], b' ' | b'\t') {
                    dir_end = c as i64 - 1;
                    break;
                }
                cs = index_of(lb, b'#', c + 1);
            }
            while dir_end > 0 && matches!(lb[dir_end as usize - 1], b' ' | b'\t') {
                dir_end -= 1;
            }
            let mut n = self.push_count(dir_end as usize);
            n += self.push_spaces(true);
            self.push_count(lb.len() - n);
            self.push_newline();
            return LexState::Stream;
        }
        if self.at_line_end() {
            let sp = self.push_spaces(true);
            self.push_count(line.len() - sp);
            self.push_newline();
            return LexState::Stream;
        }
        self.out.push(DOCUMENT);
        self.parse_line_start()
    }

    fn parse_line_start(&mut self) -> LexState {
        let ch = self.char_at(0);
        if ch == Some(b'-') || ch == Some(b'.') {
            let s = &self.b[self.pos..(self.pos + 3).min(self.b.len())];
            if (s == b"---" || s == b"...") && is_empty_ch(self.char_at(3)) {
                let is_start = s == b"---";
                self.push_count(3);
                self.indent_value = 0;
                self.indent_next = 0;
                return if is_start { LexState::Doc } else { LexState::Stream };
            }
        }
        self.indent_value = self.push_spaces(false) as i64;
        // `charAt(1)`: the character after the first, which may be multibyte.
        let first_len = self.src[self.pos..].chars().next().map_or(1, char::len_utf8);
        if self.indent_next > self.indent_value && !is_empty_ch(self.char_at(first_len)) {
            self.indent_next = self.indent_value;
        }
        self.parse_block_start()
    }

    fn parse_block_start(&mut self) -> LexState {
        let ch0 = self.char_at(0);
        let ch1 = self.char_at(1);
        if matches!(ch0, Some(b'-' | b'?' | b':')) && is_empty_ch(ch1) {
            let n = self.push_count(1) + self.push_spaces(true);
            self.indent_next = self.indent_value + 1;
            self.indent_value += n as i64;
            return LexState::BlockStart;
        }
        LexState::Doc
    }

    fn parse_document(&mut self) -> LexState {
        self.push_spaces(true);
        let line = self.get_line();
        let mut n = self.push_indicators();
        let lb = line.as_bytes();
        match lb.get(n) {
            Some(b'#') => {
                self.push_count(lb.len() - n);
                self.push_newline();
                self.parse_line_start()
            }
            None => {
                self.push_newline();
                self.parse_line_start()
            }
            Some(b'{' | b'[') => {
                self.push_count(1);
                self.flow_key = false;
                self.flow_level = 1;
                LexState::Flow
            }
            Some(b'}' | b']') => {
                self.push_count(1);
                LexState::Doc
            }
            Some(b'*') => {
                self.push_until(is_not_anchor_char);
                LexState::Doc
            }
            Some(b'"' | b'\'') => self.parse_quoted_scalar(),
            Some(b'|' | b'>') => {
                n += self.parse_block_scalar_header();
                n += self.push_spaces(true);
                self.push_count(lb.len().saturating_sub(n));
                self.push_newline();
                self.parse_block_scalar()
            }
            _ => self.parse_plain_scalar(),
        }
    }

    fn parse_flow(&mut self) -> LexState {
        let mut indent: i64 = -1;
        loop {
            let nl = self.push_newline();
            let mut sp = 0;
            if nl > 0 {
                sp = self.push_spaces(false);
                indent = sp as i64;
                self.indent_value = indent;
            }
            sp += self.push_spaces(true);
            if nl + sp == 0 {
                break;
            }
        }
        let line = self.get_line();
        let lb = line.as_bytes();
        if (indent != -1 && indent < self.indent_next && lb.first() != Some(&b'#'))
            || (indent == 0
                && (line.starts_with("---") || line.starts_with("..."))
                && is_empty_ch(lb.get(3).copied()))
        {
            let at_flow_end_marker = indent == self.indent_next - 1
                && self.flow_level == 1
                && matches!(lb.first(), Some(b']' | b'}'));
            if !at_flow_end_marker {
                self.flow_level = 0;
                self.out.push(FLOW_END);
                return self.parse_line_start();
            }
        }
        let mut n = 0;
        while lb.get(n) == Some(&b',') {
            n += self.push_count(1);
            n += self.push_spaces(true);
            self.flow_key = false;
        }
        n += self.push_indicators();
        match lb.get(n) {
            None => LexState::Flow,
            Some(b'#') => {
                self.push_count(lb.len() - n);
                LexState::Flow
            }
            Some(b'{' | b'[') => {
                self.push_count(1);
                self.flow_key = false;
                self.flow_level += 1;
                LexState::Flow
            }
            Some(b'}' | b']') => {
                self.push_count(1);
                self.flow_key = true;
                self.flow_level -= 1;
                if self.flow_level != 0 {
                    LexState::Flow
                } else {
                    LexState::Doc
                }
            }
            Some(b'*') => {
                self.push_until(is_not_anchor_char);
                LexState::Flow
            }
            Some(b'"' | b'\'') => {
                self.flow_key = true;
                self.parse_quoted_scalar()
            }
            Some(b':') => {
                let next = self.char_at(1);
                if self.flow_key || is_empty_ch(next) || next == Some(b',') {
                    self.flow_key = false;
                    self.push_count(1);
                    self.push_spaces(true);
                    return LexState::Flow;
                }
                self.flow_key = false;
                self.parse_plain_scalar()
            }
            _ => {
                self.flow_key = false;
                self.parse_plain_scalar()
            }
        }
    }

    fn parse_quoted_scalar(&mut self) -> LexState {
        let quote = self.b[self.pos];
        let mut end: i64 = index_of(self.b, quote, self.pos + 1).map_or(-1, |e| e as i64);
        if quote == b'\'' {
            while end != -1 && self.at(end + 1) == Some(b'\'') {
                end = index_of(self.b, b'\'', end as usize + 2).map_or(-1, |e| e as i64);
            }
        } else {
            while end != -1 {
                let mut n = 0;
                while self.at(end - 1 - n) == Some(b'\\') {
                    n += 1;
                }
                if n % 2 == 0 {
                    break;
                }
                end = index_of(self.b, b'"', end as usize + 1).map_or(-1, |e| e as i64);
            }
        }
        let qb = &self.b[..end.max(0) as usize];
        let mut nl = index_of(qb, b'\n', self.pos);
        if nl.is_some() {
            while let Some(n) = nl {
                let cs = self.continue_scalar(n + 1);
                if cs == -1 {
                    break;
                }
                nl = index_of(qb, b'\n', cs as usize);
            }
            if let Some(n) = nl {
                end = n as i64 - if n > 0 && qb[n - 1] == b'\r' { 2 } else { 1 };
            }
        }
        if end == -1 {
            end = self.b.len() as i64;
        }
        self.push_to_index(end as usize + 1, false);
        if self.flow_level != 0 {
            LexState::Flow
        } else {
            LexState::Doc
        }
    }

    fn parse_block_scalar_header(&mut self) -> usize {
        self.block_scalar_indent = -1;
        self.block_scalar_keep = false;
        let mut i = self.pos;
        loop {
            i += 1;
            match self.b.get(i) {
                Some(b'+') => self.block_scalar_keep = true,
                Some(c @ b'1'..=b'9') => self.block_scalar_indent = i64::from(*c - b'0') - 1,
                Some(b'-') => {}
                _ => break,
            }
        }
        self.push_until(|c| is_empty_ch(c) || c == Some(b'#'))
    }

    fn parse_block_scalar(&mut self) -> LexState {
        let mut nl: i64 = self.pos as i64 - 1;
        let mut indent: i64 = 0;
        let mut i = self.pos;
        while let Some(&ch) = self.b.get(i) {
            match ch {
                b' ' => indent += 1,
                b'\n' => {
                    nl = i as i64;
                    indent = 0;
                }
                b'\r' if self.b.get(i + 1) == Some(&b'\n') => {}
                _ => break,
            }
            i += 1;
        }
        if indent >= self.indent_next {
            if self.block_scalar_indent == -1 {
                self.indent_next = indent;
            } else {
                self.indent_next = self.block_scalar_indent + if self.indent_next == 0 { 1 } else { self.indent_next };
            }
            loop {
                let cs = self.continue_scalar((nl + 1) as usize);
                if cs == -1 {
                    break;
                }
                match index_of(self.b, b'\n', cs as usize) {
                    Some(n) => nl = n as i64,
                    None => {
                        nl = -1;
                        break;
                    }
                }
            }
            if nl == -1 {
                nl = self.b.len() as i64;
            }
        }
        let mut i = nl + 1;
        let mut ch = self.at(i);
        while ch == Some(b' ') {
            i += 1;
            ch = self.at(i);
        }
        if ch == Some(b'\t') {
            while matches!(ch, Some(b'\t' | b' ' | b'\r' | b'\n')) {
                i += 1;
                ch = self.at(i);
            }
            nl = i - 1;
        } else if !self.block_scalar_keep {
            loop {
                let mut i = nl - 1;
                let mut ch = self.at(i);
                if ch == Some(b'\r') {
                    i -= 1;
                    ch = self.at(i);
                }
                let last_char = i;
                while ch == Some(b' ') {
                    i -= 1;
                    ch = self.at(i);
                }
                if ch == Some(b'\n') && i >= self.pos as i64 && i + 1 + indent > last_char {
                    nl = i;
                } else {
                    break;
                }
            }
        }
        self.out.push(SCALAR);
        self.push_to_index((nl + 1) as usize, true);
        self.parse_line_start()
    }

    fn parse_plain_scalar(&mut self) -> LexState {
        let in_flow = self.flow_level > 0;
        let mut end: i64 = self.pos as i64 - 1;
        let mut i: i64 = self.pos as i64 - 1;
        loop {
            i += 1;
            let Some(mut ch) = self.at(i) else { break };
            if ch == b':' {
                let next = self.at(i + 1);
                if is_empty_ch(next) || (in_flow && is_flow_indicator(next)) {
                    break;
                }
                end = i;
            } else if is_empty_ch(Some(ch)) {
                let mut next = self.at(i + 1);
                if ch == b'\r' {
                    if next == Some(b'\n') {
                        i += 1;
                        ch = b'\n';
                        next = self.at(i + 1);
                    } else {
                        end = i;
                    }
                }
                if next == Some(b'#') || (in_flow && is_flow_indicator(next)) {
                    break;
                }
                if ch == b'\n' {
                    let cs = self.continue_scalar((i + 1) as usize);
                    if cs == -1 {
                        break;
                    }
                    i = i.max(cs - 2);
                }
            } else {
                if in_flow && is_flow_indicator(Some(ch)) {
                    break;
                }
                end = i;
            }
        }
        self.out.push(SCALAR);
        self.push_to_index((end + 1) as usize, true);
        if in_flow {
            LexState::Flow
        } else {
            LexState::Doc
        }
    }
}

// ---------------------------------------------------------------------------
// CST parser
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum T {
    Bom,
    DocMode,
    FlowErrorEnd,
    Scalar,
    DocStart,
    DocEnd,
    Newline,
    SeqItemInd,
    ExplicitKeyInd,
    MapValueInd,
    FlowMapStart,
    FlowMapEnd,
    FlowSeqStart,
    FlowSeqEnd,
    Comma,
    Space,
    Comment,
    DirectiveLine,
    Alias,
    Anchor,
    Tag,
    SingleQuoted,
    DoubleQuoted,
    BlockScalarHeader,
    // Composite tokens.
    Directive,
    Document,
    BlockScalar,
    BlockMap,
    BlockSeq,
    FlowCollection,
    Error,
    None,
}

impl T {
    fn name(self) -> &'static str {
        match self {
            T::Bom => "byte-order-mark",
            T::DocMode => "doc-mode",
            T::FlowErrorEnd => "flow-error-end",
            T::Scalar => "scalar",
            T::DocStart => "doc-start",
            T::DocEnd => "doc-end",
            T::Newline => "newline",
            T::SeqItemInd => "seq-item-ind",
            T::ExplicitKeyInd => "explicit-key-ind",
            T::MapValueInd => "map-value-ind",
            T::FlowMapStart => "flow-map-start",
            T::FlowMapEnd => "flow-map-end",
            T::FlowSeqStart => "flow-seq-start",
            T::FlowSeqEnd => "flow-seq-end",
            T::Comma => "comma",
            T::Space => "space",
            T::Comment => "comment",
            T::DirectiveLine => "directive-line",
            T::Alias => "alias",
            T::Anchor => "anchor",
            T::Tag => "tag",
            T::SingleQuoted => "single-quoted-scalar",
            T::DoubleQuoted => "double-quoted-scalar",
            T::BlockScalarHeader => "block-scalar-header",
            T::Directive => "directive",
            T::Document => "document",
            T::BlockScalar => "block-scalar",
            T::BlockMap => "block-map",
            T::BlockSeq => "block-seq",
            T::FlowCollection => "flow-collection",
            T::Error => "error",
            T::None => "",
        }
    }
}

fn token_type(s: &str) -> T {
    match s {
        BOM => return T::Bom,
        DOCUMENT => return T::DocMode,
        FLOW_END => return T::FlowErrorEnd,
        SCALAR => return T::Scalar,
        "---" => return T::DocStart,
        "..." => return T::DocEnd,
        "" | "\n" | "\r\n" => return T::Newline,
        "-" => return T::SeqItemInd,
        "?" => return T::ExplicitKeyInd,
        ":" => return T::MapValueInd,
        "{" => return T::FlowMapStart,
        "}" => return T::FlowMapEnd,
        "[" => return T::FlowSeqStart,
        "]" => return T::FlowSeqEnd,
        "," => return T::Comma,
        _ => {}
    }
    match s.as_bytes()[0] {
        b' ' | b'\t' => T::Space,
        b'#' => T::Comment,
        b'%' => T::DirectiveLine,
        b'*' => T::Alias,
        b'&' => T::Anchor,
        b'!' => T::Tag,
        b'\'' => T::SingleQuoted,
        b'"' => T::DoubleQuoted,
        b'|' | b'>' => T::BlockScalarHeader,
        _ => T::None,
    }
}

#[derive(Clone, Debug)]
struct Item<'a> {
    start: Vec<Tok<'a>>,
    key: Option<Box<Tok<'a>>>,
    sep: Option<Vec<Tok<'a>>>,
    value: Option<Box<Tok<'a>>>,
    explicit_key: bool,
}

impl<'a> Item<'a> {
    fn start_only(start: Vec<Tok<'a>>) -> Item<'a> {
        Item { start, key: None, sep: None, value: None, explicit_key: false }
    }
}

#[derive(Clone, Debug)]
struct Tok<'a> {
    ty: T,
    offset: usize,
    indent: i64,
    source: &'a str,
    /// Set on `Error` tokens.
    message: String,
    /// `document.start`, `flow-collection.start` (one token), `block-scalar.props`.
    start: Vec<Tok<'a>>,
    /// `document.value`.
    value: Option<Box<Tok<'a>>>,
    end: Option<Vec<Tok<'a>>>,
    items: Vec<Item<'a>>,
}

impl<'a> Tok<'a> {
    fn new(ty: T, offset: usize, indent: i64, source: &'a str) -> Tok<'a> {
        Tok { ty, offset, indent, source, message: String::new(), start: Vec::new(), value: None, end: None, items: Vec::new() }
    }

    fn has_source_len(&self) -> bool {
        !matches!(self.ty, T::BlockMap | T::BlockSeq | T::FlowCollection | T::Document)
    }

    fn pos(&self) -> (usize, usize) {
        (self.offset, self.offset + if self.has_source_len() { self.source.len() } else { 1 })
    }
}

fn includes_token(list: &[Tok<'_>], ty: T) -> bool {
    list.iter().any(|t| t.ty == ty)
}

fn find_non_empty_index(list: &[Tok<'_>]) -> Option<usize> {
    list.iter().position(|t| !matches!(t.ty, T::Space | T::Comment | T::Newline))
}

fn is_flow_token(t: Option<&Tok<'_>>) -> bool {
    matches!(
        t.map(|t| t.ty),
        Some(T::Alias | T::Scalar | T::SingleQuoted | T::DoubleQuoted | T::FlowCollection)
    )
}

fn get_first_key_start_props<'a>(prev: &mut Vec<Tok<'a>>) -> Vec<Tok<'a>> {
    if prev.is_empty() {
        return Vec::new();
    }
    let mut i = prev.len() as i64;
    loop {
        i -= 1;
        if i < 0 {
            break;
        }
        if matches!(prev[i as usize].ty, T::DocStart | T::ExplicitKeyInd | T::MapValueInd | T::SeqItemInd | T::Newline) {
            break;
        }
    }
    loop {
        i += 1;
        if prev.get(i as usize).map(|t| t.ty) != Some(T::Space) {
            break;
        }
    }
    let at = (i as usize).min(prev.len());
    prev.split_off(at)
}

fn fix_flow_seq_items<'a>(fc: &mut Tok<'a>) {
    if fc.start.first().map(|t| t.ty) != Some(T::FlowSeqStart) {
        return;
    }
    for it in &mut fc.items {
        if it.sep.is_some()
            && it.value.is_none()
            && !includes_token(&it.start, T::ExplicitKeyInd)
            && !includes_token(it.sep.as_ref().expect("sep"), T::MapValueInd)
        {
            if it.key.is_some() {
                it.value = it.key.take();
            }
            let mut sep = it.sep.take().expect("sep");
            if is_flow_token(it.value.as_deref()) {
                let v = it.value.as_mut().expect("value");
                match &mut v.end {
                    Some(e) => e.append(&mut sep),
                    None => v.end = Some(sep),
                }
            } else {
                it.start.append(&mut sep);
            }
        }
    }
}

struct Parser<'a> {
    at_new_line: bool,
    at_scalar: bool,
    indent: i64,
    offset: usize,
    on_key_line: bool,
    stack: Vec<Tok<'a>>,
    source: &'a str,
    ty: T,
    out: Vec<Tok<'a>>,
    line_starts: Vec<usize>,
}

impl<'a> Parser<'a> {
    fn parse(src: &'a str) -> (Vec<Tok<'a>>, Vec<usize>) {
        let mut p = Parser {
            at_new_line: true,
            at_scalar: false,
            indent: 0,
            offset: 0,
            on_key_line: false,
            stack: Vec::new(),
            source: "",
            ty: T::None,
            out: Vec::new(),
            line_starts: vec![0],
        };
        for lexeme in Lexer::lex(src) {
            p.next(lexeme);
        }
        while !p.stack.is_empty() {
            p.pop(None);
        }
        (p.out, p.line_starts)
    }

    fn next(&mut self, source: &'a str) {
        self.source = source;
        if self.at_scalar {
            self.at_scalar = false;
            self.step();
            self.offset += source.len();
            return;
        }
        let ty = token_type(source);
        if ty == T::None {
            let mut e = Tok::new(T::Error, self.offset, 0, source);
            e.message = format!("Not a YAML token: {source}");
            self.pop(Some(e));
            self.offset += source.len();
        } else if ty == T::Scalar {
            self.at_new_line = false;
            self.at_scalar = true;
            self.ty = T::Scalar;
        } else {
            self.ty = ty;
            self.step();
            match ty {
                T::Newline => {
                    self.at_new_line = true;
                    self.indent = 0;
                    self.line_starts.push(self.offset + source.len());
                }
                T::Space => {
                    if self.at_new_line && source.as_bytes()[0] == b' ' {
                        self.indent += source.len() as i64;
                    }
                }
                T::ExplicitKeyInd | T::MapValueInd | T::SeqItemInd => {
                    if self.at_new_line {
                        self.indent += source.len() as i64;
                    }
                }
                T::DocMode | T::FlowErrorEnd => return,
                _ => self.at_new_line = false,
            }
            self.offset += source.len();
        }
    }

    fn source_token(&self) -> Tok<'a> {
        Tok::new(self.ty, self.offset, self.indent, self.source)
    }

    fn step(&mut self) {
        let top = self.stack.last().map(|t| t.ty);
        if self.ty == T::DocEnd && top != Some(T::DocEnd) {
            while !self.stack.is_empty() {
                self.pop(None);
            }
            self.stack.push(Tok::new(T::DocEnd, self.offset, 0, self.source));
            return;
        }
        let Some(top) = top else {
            self.stream();
            return;
        };
        match top {
            T::Document => self.document(),
            T::Alias | T::Scalar | T::SingleQuoted | T::DoubleQuoted => self.scalar(),
            T::BlockScalar => self.block_scalar(),
            T::BlockMap => self.block_map(),
            T::BlockSeq => self.block_sequence(),
            T::FlowCollection => self.flow_collection(),
            T::DocEnd => self.document_end(),
            _ => self.pop(None),
        }
    }

    fn pop(&mut self, error: Option<Tok<'a>>) {
        let token = match error {
            Some(e) => Some(e),
            None => self.stack.pop(),
        };
        let Some(mut token) = token else {
            let mut e = Tok::new(T::Error, self.offset, 0, "");
            e.message = "Tried to pop an empty stack".into();
            self.out.push(e);
            return;
        };
        if self.stack.is_empty() {
            self.out.push(token);
            return;
        }
        let top_i = self.stack.len() - 1;
        let top_ty = self.stack[top_i].ty;
        if token.ty == T::BlockScalar {
            token.indent = if top_ty == T::Document { 0 } else { self.stack[top_i].indent };
        } else if token.ty == T::FlowCollection && top_ty == T::Document {
            token.indent = 0;
        }
        if token.ty == T::FlowCollection {
            fix_flow_seq_items(&mut token);
        }
        // The fields read after `token` moves into the tree.
        let token_ty = token.ty;
        let token_indent = token.indent;
        let mut moved_last = None;
        // A token that becomes a map key returns before upstream's trailing check.
        let becomes_value = match top_ty {
            T::Document | T::BlockSeq => true,
            T::BlockMap => {
                let it = self.stack[top_i].items.last().expect("item");
                it.value.is_none() && it.sep.is_some()
            }
            _ => false,
        };
        if becomes_value && matches!(token_ty, T::BlockMap | T::BlockSeq) {
            let last = token.items.last();
            if let Some(last) = last {
                if last.sep.is_none()
                    && last.value.is_none()
                    && !last.start.is_empty()
                    && find_non_empty_index(&last.start).is_none()
                    && (token_indent == 0
                        || last.start.iter().all(|st| st.ty != T::Comment || st.indent < token_indent))
                {
                    moved_last = Some(token.items.pop().expect("last").start);
                }
            }
        }
        match top_ty {
            T::Document => self.stack[top_i].value = Some(Box::new(token)),
            T::BlockScalar => self.stack[top_i].start.push(token),
            T::BlockMap => {
                let items = &mut self.stack[top_i].items;
                let it = items.last_mut().expect("item");
                if it.value.is_some() {
                    items.push(Item {
                        start: Vec::new(),
                        key: Some(Box::new(token)),
                        sep: Some(Vec::new()),
                        value: None,
                        explicit_key: false,
                    });
                    self.on_key_line = true;
                    return;
                } else if it.sep.is_some() {
                    it.value = Some(Box::new(token));
                } else {
                    it.key = Some(Box::new(token));
                    it.sep = Some(Vec::new());
                    self.on_key_line = !it.explicit_key;
                    return;
                }
            }
            T::BlockSeq => {
                let items = &mut self.stack[top_i].items;
                let it = items.last_mut().expect("item");
                if it.value.is_some() {
                    let mut item = Item::start_only(Vec::new());
                    item.value = Some(Box::new(token));
                    items.push(item);
                } else {
                    it.value = Some(Box::new(token));
                }
            }
            T::FlowCollection => {
                let items = &mut self.stack[top_i].items;
                match items.last_mut() {
                    Some(it) if it.value.is_none() => {
                        if it.sep.is_some() {
                            it.value = Some(Box::new(token));
                        } else {
                            it.key = Some(Box::new(token));
                            it.sep = Some(Vec::new());
                        }
                    }
                    _ => items.push(Item {
                        start: Vec::new(),
                        key: Some(Box::new(token)),
                        sep: Some(Vec::new()),
                        value: None,
                        explicit_key: false,
                    }),
                }
                return;
            }
            _ => {
                self.pop(None);
                self.pop(Some(token));
            }
        }
        if let Some(start) = moved_last {
            // `top` here is the parent the token was just attached to.
            let top = &mut self.stack[top_i];
            if top.ty == T::Document {
                top.end = Some(start);
            } else {
                top.items.push(Item::start_only(start));
            }
        }
    }

    fn stream(&mut self) {
        match self.ty {
            T::DirectiveLine => {
                self.out.push(Tok::new(T::Directive, self.offset, 0, self.source));
                return;
            }
            T::Bom | T::Space | T::Comment | T::Newline => {
                let t = self.source_token();
                self.out.push(t);
                return;
            }
            T::DocMode | T::DocStart => {
                let mut doc = Tok::new(T::Document, self.offset, 0, "");
                if self.ty == T::DocStart {
                    doc.start.push(self.source_token());
                }
                self.stack.push(doc);
                return;
            }
            _ => {}
        }
        let mut e = Tok::new(T::Error, self.offset, 0, self.source);
        e.message = format!("Unexpected {} token in YAML stream", self.ty.name());
        self.out.push(e);
    }

    fn document(&mut self) {
        let i = self.stack.len() - 1;
        if self.stack[i].value.is_some() {
            self.line_end();
            return;
        }
        match self.ty {
            T::DocStart => {
                if find_non_empty_index(&self.stack[i].start).is_some() {
                    self.pop(None);
                    self.step();
                } else {
                    let t = self.source_token();
                    self.stack[i].start.push(t);
                }
                return;
            }
            T::Anchor | T::Tag | T::Space | T::Comment | T::Newline => {
                let t = self.source_token();
                self.stack[i].start.push(t);
                return;
            }
            _ => {}
        }
        match self.start_block_value(i) {
            Some(bv) => self.stack.push(bv),
            None => {
                let mut e = Tok::new(T::Error, self.offset, 0, self.source);
                e.message = format!("Unexpected {} token in YAML document", self.ty.name());
                self.out.push(e);
            }
        }
    }

    /// `getPrevProps(parent)` for the stack entry at `i`.
    fn prev_props(&mut self, i: usize) -> Option<&mut Vec<Tok<'a>>> {
        let parent = &mut self.stack[i];
        match parent.ty {
            T::Document => Some(&mut parent.start),
            T::BlockMap => {
                let it = parent.items.last_mut().expect("item");
                Some(match &mut it.sep {
                    Some(sep) => sep,
                    None => &mut it.start,
                })
            }
            T::BlockSeq => Some(&mut parent.items.last_mut().expect("item").start),
            _ => None,
        }
    }

    fn take_first_key_start(&mut self, i: Option<usize>) -> Vec<Tok<'a>> {
        match i.and_then(|i| self.prev_props(i)) {
            Some(prev) => get_first_key_start_props(prev),
            None => Vec::new(),
        }
    }

    fn scalar(&mut self) {
        if self.ty == T::MapValueInd {
            let parent = self.stack.len().checked_sub(2);
            let start = self.take_first_key_start(parent);
            let mut scalar = self.stack.pop().expect("scalar");
            let mut sep = scalar.end.take().unwrap_or_default();
            sep.push(self.source_token());
            let mut map = Tok::new(T::BlockMap, scalar.offset, scalar.indent, "");
            map.items.push(Item {
                start,
                key: Some(Box::new(scalar)),
                sep: Some(sep),
                value: None,
                explicit_key: false,
            });
            self.on_key_line = true;
            self.stack.push(map);
        } else {
            self.line_end();
        }
    }

    fn block_scalar(&mut self) {
        match self.ty {
            T::Space | T::Comment | T::Newline => {
                let t = self.source_token();
                self.stack.last_mut().expect("top").start.push(t);
            }
            T::Scalar => {
                self.stack.last_mut().expect("top").source = self.source;
                self.at_new_line = true;
                self.indent = 0;
                let (b, off) = (self.source.as_bytes(), self.offset);
                for (k, c) in b.iter().enumerate() {
                    if *c == b'\n' {
                        self.line_starts.push(off + k + 1);
                    }
                }
                self.pop(None);
            }
            _ => {
                self.pop(None);
                self.step();
            }
        }
    }

    fn at_indented_comment(&self, start: &[Tok<'_>], indent: i64) -> bool {
        self.ty == T::Comment
            && self.indent > indent
            && start.iter().all(|st| matches!(st.ty, T::Newline | T::Space))
    }

    fn block_map(&mut self) {
        let m = self.stack.len() - 1;
        let map_indent = self.stack[m].indent;
        let tok = self.source_token();
        match self.ty {
            T::Newline => {
                self.on_key_line = false;
                let map = &mut self.stack[m];
                let it = map.items.last_mut().expect("item");
                if it.value.is_some() {
                    let end = it.value.as_mut().and_then(|v| v.end.as_mut());
                    if let Some(end) = end.filter(|e| e.last().map(|t| t.ty) == Some(T::Comment)) {
                        end.push(tok);
                    } else {
                        map.items.push(Item::start_only(vec![tok]));
                    }
                } else if let Some(sep) = it.sep.as_mut() {
                    sep.push(tok);
                } else {
                    it.start.push(tok);
                }
                return;
            }
            T::Space | T::Comment => {
                let it_has_value = self.stack[m].items.last().expect("item").value.is_some();
                let it_has_sep = self.stack[m].items.last().expect("item").sep.is_some();
                if it_has_value {
                    self.stack[m].items.push(Item::start_only(vec![tok]));
                } else if it_has_sep {
                    self.stack[m].items.last_mut().expect("item").sep.as_mut().expect("sep").push(tok);
                } else {
                    let at_ind = {
                        let it = self.stack[m].items.last().expect("item");
                        self.at_indented_comment(&it.start, map_indent)
                    };
                    if at_ind {
                        let n = self.stack[m].items.len();
                        let has_end = n >= 2
                            && self.stack[m].items[n - 2].value.as_ref().is_some_and(|v| v.end.is_some());
                        if has_end {
                            let it = self.stack[m].items.pop().expect("item");
                            let mut st = it.start;
                            let prev = self.stack[m].items.last_mut().expect("prev");
                            let end = prev.value.as_mut().expect("value").end.as_mut().expect("end");
                            end.append(&mut st);
                            end.push(tok);
                            return;
                        }
                    }
                    self.stack[m].items.last_mut().expect("item").start.push(tok);
                }
                return;
            }
            _ => {}
        }
        if self.indent >= map_indent {
            let at_map_indent = !self.on_key_line && self.indent == map_indent;
            let (has_sep, has_value, explicit_key) = {
                let it = self.stack[m].items.last().expect("item");
                (it.sep.is_some(), it.value.is_some(), it.explicit_key)
            };
            let at_next_item = at_map_indent && (has_sep || explicit_key) && self.ty != T::SeqItemInd;
            let mut start: Vec<Tok<'a>> = Vec::new();
            if at_next_item && has_sep && !has_value {
                let sep = self.stack[m].items.last_mut().expect("item").sep.as_mut().expect("sep");
                let mut nl: Vec<usize> = Vec::new();
                for (i, st) in sep.iter().enumerate() {
                    match st.ty {
                        T::Newline => nl.push(i),
                        T::Space => {}
                        T::Comment => {
                            if st.indent > map_indent {
                                nl.clear();
                            }
                        }
                        _ => nl.clear(),
                    }
                }
                if nl.len() >= 2 {
                    start = sep.split_off(nl[1]);
                }
            }
            match self.ty {
                T::Anchor | T::Tag => {
                    if at_next_item || has_value {
                        start.push(tok);
                        self.stack[m].items.push(Item::start_only(start));
                        self.on_key_line = true;
                    } else if has_sep {
                        self.stack[m].items.last_mut().expect("item").sep.as_mut().expect("sep").push(tok);
                    } else {
                        self.stack[m].items.last_mut().expect("item").start.push(tok);
                    }
                    return;
                }
                T::ExplicitKeyInd => {
                    if !has_sep && !explicit_key {
                        let it = self.stack[m].items.last_mut().expect("item");
                        it.start.push(tok);
                        it.explicit_key = true;
                    } else if at_next_item || has_value {
                        start.push(tok);
                        let mut item = Item::start_only(start);
                        item.explicit_key = true;
                        self.stack[m].items.push(item);
                    } else {
                        let mut bm = Tok::new(T::BlockMap, self.offset, self.indent, "");
                        let mut item = Item::start_only(vec![tok]);
                        item.explicit_key = true;
                        bm.items.push(item);
                        self.stack.push(bm);
                    }
                    self.on_key_line = true;
                    return;
                }
                T::MapValueInd => {
                    let null_key = |start: Vec<Tok<'a>>, sep: Vec<Tok<'a>>| Item {
                        start,
                        key: None,
                        sep: Some(sep),
                        value: None,
                        explicit_key: false,
                    };
                    if explicit_key {
                        if !has_sep {
                            let it = self.stack[m].items.last_mut().expect("item");
                            if includes_token(&it.start, T::Newline) {
                                it.key = None;
                                it.sep = Some(vec![tok]);
                            } else {
                                let start = get_first_key_start_props(&mut it.start);
                                let mut bm = Tok::new(T::BlockMap, self.offset, self.indent, "");
                                bm.items.push(null_key(start, vec![tok]));
                                self.stack.push(bm);
                            }
                        } else if has_value {
                            self.stack[m].items.push(null_key(Vec::new(), vec![tok]));
                        } else {
                            let it = self.stack[m].items.last_mut().expect("item");
                            let sep_has_ind = includes_token(it.sep.as_ref().expect("sep"), T::MapValueInd);
                            if sep_has_ind {
                                let mut bm = Tok::new(T::BlockMap, self.offset, self.indent, "");
                                bm.items.push(null_key(start, vec![tok]));
                                self.stack.push(bm);
                            } else if is_flow_token(it.key.as_deref())
                                && !includes_token(it.sep.as_ref().expect("sep"), T::Newline)
                            {
                                let st = get_first_key_start_props(&mut it.start);
                                let key = it.key.take();
                                let mut sep = it.sep.take().expect("sep");
                                sep.push(tok);
                                let mut bm = Tok::new(T::BlockMap, self.offset, self.indent, "");
                                bm.items.push(Item { start: st, key, sep: Some(sep), value: None, explicit_key: false });
                                self.stack.push(bm);
                            } else {
                                let sep = it.sep.as_mut().expect("sep");
                                sep.append(&mut start);
                                sep.push(tok);
                            }
                        }
                    } else if !has_sep {
                        let it = self.stack[m].items.last_mut().expect("item");
                        it.key = None;
                        it.sep = Some(vec![tok]);
                    } else if has_value || at_next_item {
                        self.stack[m].items.push(null_key(start, vec![tok]));
                    } else {
                        let it = self.stack[m].items.last_mut().expect("item");
                        if includes_token(it.sep.as_ref().expect("sep"), T::MapValueInd) {
                            let mut bm = Tok::new(T::BlockMap, self.offset, self.indent, "");
                            bm.items.push(null_key(Vec::new(), vec![tok]));
                            self.stack.push(bm);
                        } else {
                            it.sep.as_mut().expect("sep").push(tok);
                        }
                    }
                    self.on_key_line = true;
                    return;
                }
                T::Alias | T::Scalar | T::SingleQuoted | T::DoubleQuoted => {
                    let fs = self.flow_scalar(self.ty);
                    if at_next_item || has_value {
                        self.stack[m].items.push(Item {
                            start,
                            key: Some(Box::new(fs)),
                            sep: Some(Vec::new()),
                            value: None,
                            explicit_key: false,
                        });
                        self.on_key_line = true;
                    } else if has_sep {
                        self.stack.push(fs);
                    } else {
                        let it = self.stack[m].items.last_mut().expect("item");
                        it.key = Some(Box::new(fs));
                        it.sep = Some(Vec::new());
                        self.on_key_line = true;
                    }
                    return;
                }
                _ => {
                    if let Some(bv) = self.start_block_value(m) {
                        if bv.ty == T::BlockSeq {
                            let it = self.stack[m].items.last().expect("item");
                            if !it.explicit_key
                                && it.sep.as_ref().is_some_and(|s| !includes_token(s, T::Newline))
                            {
                                let mut e = Tok::new(T::Error, self.offset, 0, self.source);
                                e.message = "Unexpected block-seq-ind on same line with key".into();
                                self.pop(Some(e));
                                return;
                            }
                        } else if at_map_indent {
                            self.stack[m].items.push(Item::start_only(start));
                        }
                        self.stack.push(bv);
                        return;
                    }
                }
            }
        }
        self.pop(None);
        self.step();
    }

    fn block_sequence(&mut self) {
        let s = self.stack.len() - 1;
        let seq_indent = self.stack[s].indent;
        let tok = self.source_token();
        match self.ty {
            T::Newline => {
                let seq = &mut self.stack[s];
                let it = seq.items.last_mut().expect("item");
                if it.value.is_some() {
                    let end = it.value.as_mut().and_then(|v| v.end.as_mut());
                    if let Some(end) = end.filter(|e| e.last().map(|t| t.ty) == Some(T::Comment)) {
                        end.push(tok);
                    } else {
                        seq.items.push(Item::start_only(vec![tok]));
                    }
                } else {
                    it.start.push(tok);
                }
                return;
            }
            T::Space | T::Comment => {
                if self.stack[s].items.last().expect("item").value.is_some() {
                    self.stack[s].items.push(Item::start_only(vec![tok]));
                } else {
                    let at_ind = {
                        let it = self.stack[s].items.last().expect("item");
                        self.at_indented_comment(&it.start, seq_indent)
                    };
                    if at_ind {
                        let n = self.stack[s].items.len();
                        let has_end = n >= 2
                            && self.stack[s].items[n - 2].value.as_ref().is_some_and(|v| v.end.is_some());
                        if has_end {
                            let it = self.stack[s].items.pop().expect("item");
                            let mut st = it.start;
                            let prev = self.stack[s].items.last_mut().expect("prev");
                            let end = prev.value.as_mut().expect("value").end.as_mut().expect("end");
                            end.append(&mut st);
                            end.push(tok);
                            return;
                        }
                    }
                    self.stack[s].items.last_mut().expect("item").start.push(tok);
                }
                return;
            }
            T::Anchor | T::Tag => {
                let it = self.stack[s].items.last_mut().expect("item");
                if !(it.value.is_some() || self.indent <= seq_indent) {
                    it.start.push(tok);
                    return;
                }
            }
            T::SeqItemInd if self.indent == seq_indent => {
                let seq = &mut self.stack[s];
                let it = seq.items.last_mut().expect("item");
                if it.value.is_some() || includes_token(&it.start, T::SeqItemInd) {
                    seq.items.push(Item::start_only(vec![tok]));
                } else {
                    it.start.push(tok);
                }
                return;
            }
            _ => {}
        }
        if self.indent > seq_indent {
            if let Some(bv) = self.start_block_value(s) {
                self.stack.push(bv);
                return;
            }
        }
        self.pop(None);
        self.step();
    }

    fn flow_collection(&mut self) {
        let f = self.stack.len() - 1;
        if self.ty == T::FlowErrorEnd {
            loop {
                self.pop(None);
                if self.stack.last().map(|t| t.ty) != Some(T::FlowCollection) {
                    break;
                }
            }
            return;
        }
        let end_empty = self.stack[f].end.as_ref().is_none_or(Vec::is_empty);
        if end_empty {
            let tok = self.source_token();
            let items = &mut self.stack[f].items;
            let last = items.last().map(|it| (it.sep.is_some(), it.value.is_some()));
            match self.ty {
                T::Comma | T::ExplicitKeyInd => {
                    match last {
                        None | Some((true, _)) => items.push(Item::start_only(vec![tok])),
                        _ => items.last_mut().expect("item").start.push(tok),
                    }
                    return;
                }
                T::MapValueInd => {
                    match last {
                        None | Some((_, true)) => items.push(Item {
                            start: Vec::new(),
                            key: None,
                            sep: Some(vec![tok]),
                            value: None,
                            explicit_key: false,
                        }),
                        Some((true, _)) => items.last_mut().expect("item").sep.as_mut().expect("sep").push(tok),
                        _ => {
                            let it = items.last_mut().expect("item");
                            it.key = None;
                            it.sep = Some(vec![tok]);
                        }
                    }
                    return;
                }
                T::Space | T::Comment | T::Newline | T::Anchor | T::Tag => {
                    match last {
                        None | Some((_, true)) => items.push(Item::start_only(vec![tok])),
                        Some((true, _)) => items.last_mut().expect("item").sep.as_mut().expect("sep").push(tok),
                        _ => items.last_mut().expect("item").start.push(tok),
                    }
                    return;
                }
                T::Alias | T::Scalar | T::SingleQuoted | T::DoubleQuoted => {
                    let fs = self.flow_scalar(self.ty);
                    let items = &mut self.stack[f].items;
                    match last {
                        None | Some((_, true)) => items.push(Item {
                            start: Vec::new(),
                            key: Some(Box::new(fs)),
                            sep: Some(Vec::new()),
                            value: None,
                            explicit_key: false,
                        }),
                        Some((true, _)) => self.stack.push(fs),
                        _ => {
                            let it = items.last_mut().expect("item");
                            it.key = Some(Box::new(fs));
                            it.sep = Some(Vec::new());
                        }
                    }
                    return;
                }
                T::FlowMapEnd | T::FlowSeqEnd => {
                    self.stack[f].end.get_or_insert_with(Vec::new).push(tok);
                    return;
                }
                _ => {}
            }
            match self.start_block_value(f) {
                Some(bv) => self.stack.push(bv),
                None => {
                    self.pop(None);
                    self.step();
                }
            }
        } else {
            let parent_i = f.checked_sub(1);
            let parent_ty = parent_i.map(|i| self.stack[i].ty);
            let fc_indent = self.stack[f].indent;
            let parent_cond = parent_i.is_some_and(|i| {
                let parent = &self.stack[i];
                parent.ty == T::BlockMap
                    && ((self.ty == T::MapValueInd && parent.indent == fc_indent)
                        || (self.ty == T::Newline && parent.items.last().is_some_and(|it| it.sep.is_none())))
            });
            if parent_cond {
                self.pop(None);
                self.step();
            } else if self.ty == T::MapValueInd && parent_ty != Some(T::FlowCollection) {
                let start = self.take_first_key_start(parent_i);
                let mut fc = self.stack.pop().expect("fc");
                fix_flow_seq_items(&mut fc);
                let mut sep: Vec<Tok<'a>> = fc.end.as_mut().expect("end").split_off(1);
                sep.push(self.source_token());
                let mut map = Tok::new(T::BlockMap, fc.offset, fc.indent, "");
                map.items.push(Item { start, key: Some(Box::new(fc)), sep: Some(sep), value: None, explicit_key: false });
                self.on_key_line = true;
                self.stack.push(map);
            } else {
                self.line_end();
            }
        }
    }

    fn flow_scalar(&mut self, ty: T) -> Tok<'a> {
        for (k, c) in self.source.as_bytes().iter().enumerate() {
            if *c == b'\n' {
                self.line_starts.push(self.offset + k + 1);
            }
        }
        Tok::new(ty, self.offset, self.indent, self.source)
    }

    fn start_block_value(&mut self, parent: usize) -> Option<Tok<'a>> {
        match self.ty {
            T::Alias | T::Scalar | T::SingleQuoted | T::DoubleQuoted => Some(self.flow_scalar(self.ty)),
            T::BlockScalarHeader => {
                let mut t = Tok::new(T::BlockScalar, self.offset, self.indent, "");
                t.start.push(self.source_token());
                Some(t)
            }
            T::FlowMapStart | T::FlowSeqStart => {
                let mut t = Tok::new(T::FlowCollection, self.offset, self.indent, "");
                t.start.push(self.source_token());
                t.end = Some(Vec::new());
                Some(t)
            }
            T::SeqItemInd => {
                let mut t = Tok::new(T::BlockSeq, self.offset, self.indent, "");
                t.items.push(Item::start_only(vec![self.source_token()]));
                Some(t)
            }
            T::ExplicitKeyInd => {
                self.on_key_line = true;
                let mut start = self.take_first_key_start(Some(parent));
                start.push(self.source_token());
                let mut t = Tok::new(T::BlockMap, self.offset, self.indent, "");
                let mut item = Item::start_only(start);
                item.explicit_key = true;
                t.items.push(item);
                Some(t)
            }
            T::MapValueInd => {
                self.on_key_line = true;
                let start = self.take_first_key_start(Some(parent));
                let mut t = Tok::new(T::BlockMap, self.offset, self.indent, "");
                t.items.push(Item {
                    start,
                    key: None,
                    sep: Some(vec![self.source_token()]),
                    value: None,
                    explicit_key: false,
                });
                Some(t)
            }
            _ => None,
        }
    }

    fn document_end(&mut self) {
        if self.ty != T::DocMode {
            let t = self.source_token();
            self.stack.last_mut().expect("top").end.get_or_insert_with(Vec::new).push(t);
            if self.ty == T::Newline {
                self.pop(None);
            }
        }
    }

    fn line_end(&mut self) {
        match self.ty {
            T::Comma | T::DocStart | T::DocEnd | T::FlowSeqEnd | T::FlowMapEnd | T::MapValueInd => {
                self.pop(None);
                self.step();
            }
            _ => {
                if self.ty == T::Newline {
                    self.on_key_line = false;
                }
                let t = self.source_token();
                self.stack.last_mut().expect("top").end.get_or_insert_with(Vec::new).push(t);
                if self.ty == T::Newline {
                    self.pop(None);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Composer
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct ParseErr {
    pos: (usize, usize),
    code: &'static str,
    message: String,
}

#[derive(Debug)]
enum Sv {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
}

#[derive(Debug)]
enum Nk {
    Scalar(Sv),
    Map(Vec<(Node, Option<Node>)>),
    Seq(Vec<Node>),
    Alias(String),
}

#[derive(Debug)]
struct Node {
    kind: Nk,
    anchor: Option<String>,
    range: [usize; 3],
}

#[derive(Clone)]
struct Directives {
    tags: Vec<(String, String)>,
    explicit: bool,
    version: String,
    at_next_document: bool,
}

impl Directives {
    fn new() -> Directives {
        Directives {
            tags: vec![("!!".into(), "tag:yaml.org,2002:".into())],
            explicit: false,
            version: "1.2".into(),
            at_next_document: false,
        }
    }

    fn at_document(&mut self) -> Directives {
        let res = self.clone();
        if self.version == "1.1" {
            self.at_next_document = true;
        } else if self.version == "1.2" {
            self.at_next_document = false;
            self.explicit = false;
            self.tags = Directives::new().tags;
        }
        res
    }

    fn add(&mut self, line: &str, mut on_error: impl FnMut(usize, String)) {
        if self.at_next_document {
            self.explicit = false;
            self.version = "1.1".into();
            self.tags = Directives::new().tags;
            self.at_next_document = false;
        }
        let trimmed = line.trim_matches(js::is_js_whitespace);
        let mut parts = trimmed.split([' ', '\t']).filter(|p| !p.is_empty());
        let name = parts.next().unwrap_or("");
        let parts: Vec<&str> = parts.collect();
        match name {
            "%TAG" => {
                if parts.len() != 2 {
                    on_error(0, "%TAG directive should contain exactly two parts".into());
                    if parts.len() < 2 {
                        return;
                    }
                }
                let (handle, prefix) = (parts[0].to_string(), parts[1].to_string());
                match self.tags.iter_mut().find(|t| t.0 == handle) {
                    Some(t) => t.1 = prefix,
                    None => self.tags.push((handle, prefix)),
                }
            }
            "%YAML" => {
                self.explicit = true;
                if parts.len() != 1 {
                    on_error(0, "%YAML directive should contain exactly one part".into());
                    return;
                }
                let version = parts[0];
                if version == "1.1" || version == "1.2" {
                    self.version = version.into();
                } else {
                    let valid = version.split_once('.').is_some_and(|(a, b)| {
                        !a.is_empty()
                            && !b.is_empty()
                            && a.bytes().all(|c| c.is_ascii_digit())
                            && b.bytes().all(|c| c.is_ascii_digit())
                    });
                    if !valid {
                        on_error(6, format!("Unsupported YAML version {version}"));
                    }
                }
            }
            _ => {}
        }
    }

    /// `tagName(source, onError)`: `None` is upstream's `null`.
    fn tag_name(&self, source: &str, mut on_error: impl FnMut(String)) -> Option<String> {
        if source == "!" {
            return Some("!".into());
        }
        if !source.starts_with('!') {
            on_error(format!("Not a valid tag: {source}"));
            return None;
        }
        if source.as_bytes().get(1) == Some(&b'<') {
            let verbatim = &source[2..source.len().saturating_sub(1).max(2)];
            if verbatim == "!" || verbatim == "!!" {
                on_error(format!("Verbatim tags aren't resolved, so {source} is invalid."));
                return None;
            }
            if !source.ends_with('>') {
                on_error("Verbatim tags must end with a >".into());
            }
            return Some(verbatim.to_string());
        }
        let split = source.rfind('!').expect("starts with !") + 1;
        let (handle, suffix) = source.split_at(split);
        if suffix.is_empty() {
            on_error(format!("The {source} tag has no suffix"));
        }
        if let Some((_, prefix)) = self.tags.iter().find(|t| t.0 == handle) {
            return match lq_json::uri::decode_uri_component(suffix) {
                Some(d) => Some(format!("{prefix}{d}")),
                None => {
                    on_error("URIError: URI malformed".into());
                    None
                }
            };
        }
        if handle == "!" {
            return Some(source.to_string());
        }
        on_error(format!("Could not resolve tag: {source}"));
        None
    }
}

struct PTok<'a> {
    ty: T,
    offset: usize,
    indent: i64,
    source: &'a str,
}

impl<'a> PTok<'a> {
    fn of(t: &Tok<'a>) -> PTok<'a> {
        PTok { ty: t.ty, offset: t.offset, indent: t.indent, source: t.source }
    }

    fn pos(&self) -> (usize, usize) {
        (self.offset, self.offset + self.source.len())
    }
}

#[derive(Default)]
struct Props<'a> {
    comma: Option<PTok<'a>>,
    found: Option<PTok<'a>>,
    has_comment: bool,
    has_newline: bool,
    anchor: Option<PTok<'a>>,
    tag: Option<PTok<'a>>,
    newline_after_prop: Option<PTok<'a>>,
    end: usize,
    start: usize,
}

struct RpOpts<'a, 'b> {
    flow: Option<&'static str>,
    indicator: T,
    next: Option<&'b Tok<'a>>,
    offset: usize,
    parent_indent: i64,
    start_on_newline: bool,
}

struct Ctx<'d> {
    at_key: bool,
    at_root: bool,
    directives: &'d Directives,
    depth: usize,
}

const MAX_DEPTH: usize = 2000;

struct Composer {
    errors: Vec<ParseErr>,
}

impl Composer {
    fn err(&mut self, pos: (usize, usize), code: &'static str, message: impl Into<String>) {
        self.errors.push(ParseErr { pos, code, message: message.into() });
    }

    fn err_at(&mut self, offset: usize, code: &'static str, message: impl Into<String>) {
        self.err((offset, offset + 1), code, message);
    }

    fn resolve_props<'a>(&mut self, tokens: &[Tok<'a>], o: RpOpts<'a, '_>) -> Props<'a> {
        let mut at_newline = o.start_on_newline;
        let mut has_space = o.start_on_newline;
        let mut p = Props::default();
        let mut req_space = false;
        let mut tab: Option<PTok<'a>> = None;
        let mut start: Option<usize> = None;
        for token in tokens {
            if req_space {
                if !matches!(token.ty, T::Space | T::Newline | T::Comma) {
                    self.err_at(token.offset, "MISSING_CHAR", "Tags and anchors must be separated from the next token by white space");
                }
                req_space = false;
            }
            if let Some(t) = tab.take() {
                if at_newline && !matches!(token.ty, T::Comment | T::Newline) {
                    self.err(t.pos(), "TAB_AS_INDENT", "Tabs are not allowed as indentation");
                }
            }
            match token.ty {
                T::Space => {
                    if o.flow.is_none()
                        && (o.indicator != T::DocStart || o.next.map(|n| n.ty) != Some(T::FlowCollection))
                        && token.source.contains('\t')
                    {
                        tab = Some(PTok::of(token));
                    }
                    has_space = true;
                }
                T::Comment => {
                    if !has_space {
                        self.err(token.pos(), "MISSING_CHAR", "Comments must be separated from other tokens by white space characters");
                    }
                    p.has_comment = true;
                    at_newline = false;
                }
                T::Newline => {
                    at_newline = true;
                    p.has_newline = true;
                    if p.anchor.is_some() || p.tag.is_some() {
                        p.newline_after_prop = Some(PTok::of(token));
                    }
                    has_space = true;
                }
                T::Anchor => {
                    if p.anchor.is_some() {
                        self.err(token.pos(), "MULTIPLE_ANCHORS", "A node can have at most one anchor");
                    }
                    p.anchor = Some(PTok::of(token));
                    start.get_or_insert(token.offset);
                    at_newline = false;
                    has_space = false;
                    req_space = true;
                }
                T::Tag => {
                    if p.tag.is_some() {
                        self.err(token.pos(), "MULTIPLE_TAGS", "A node can have at most one tag");
                    }
                    p.tag = Some(PTok::of(token));
                    start.get_or_insert(token.offset);
                    at_newline = false;
                    has_space = false;
                    req_space = true;
                }
                ty if ty == o.indicator => {
                    if p.anchor.is_some() || p.tag.is_some() {
                        self.err(token.pos(), "BAD_PROP_ORDER", format!("Anchors and tags must be after the {} indicator", token.source));
                    }
                    if p.found.is_some() {
                        self.err(token.pos(), "UNEXPECTED_TOKEN", format!("Unexpected {} in {}", token.source, o.flow.unwrap_or("collection")));
                    }
                    p.found = Some(PTok::of(token));
                    at_newline = matches!(o.indicator, T::SeqItemInd | T::ExplicitKeyInd);
                    has_space = false;
                }
                T::Comma if o.flow.is_some() => {
                    if p.comma.is_some() {
                        self.err(token.pos(), "UNEXPECTED_TOKEN", format!("Unexpected , in {}", o.flow.unwrap_or("")));
                    }
                    p.comma = Some(PTok::of(token));
                    at_newline = false;
                    has_space = false;
                }
                _ => {
                    self.err(token.pos(), "UNEXPECTED_TOKEN", format!("Unexpected {} token", token.ty.name()));
                    at_newline = false;
                    has_space = false;
                }
            }
        }
        let end = tokens.last().map_or(o.offset, |l| l.offset + l.source.len());
        if req_space {
            if let Some(next) = o.next {
                if !matches!(next.ty, T::Space | T::Newline | T::Comma)
                    && (next.ty != T::Scalar || !next.source.is_empty())
                {
                    self.err_at(next.offset, "MISSING_CHAR", "Tags and anchors must be separated from the next token by white space");
                }
            }
        }
        if let Some(t) = tab {
            if (at_newline && t.indent <= o.parent_indent)
                || matches!(o.next.map(|n| n.ty), Some(T::BlockMap | T::BlockSeq))
            {
                self.err(t.pos(), "TAB_AS_INDENT", "Tabs are not allowed as indentation");
            }
        }
        p.end = end;
        p.start = start.unwrap_or(end);
        p
    }

    fn compose_node<'a>(&mut self, ctx: &mut Ctx<'_>, token: &Tok<'a>, props: &Props<'a>) -> Node {
        let anchor = props.anchor.as_ref();
        let tag = props.tag.as_ref();
        let mut node: Option<Node> = None;
        match token.ty {
            T::Alias => {
                node = Some(self.compose_alias(token));
                if anchor.is_some() || tag.is_some() {
                    self.err(token.pos(), "ALIAS_PROPS", "An alias node must not specify any properties");
                }
            }
            T::Scalar | T::SingleQuoted | T::DoubleQuoted | T::BlockScalar => {
                let mut n = self.compose_scalar(ctx, token, tag);
                if let Some(a) = anchor {
                    n.anchor = Some(a.source[1..].to_string());
                }
                node = Some(n);
            }
            T::BlockMap | T::BlockSeq | T::FlowCollection => {
                ctx.depth += 1;
                if ctx.depth > MAX_DEPTH {
                    self.err(token.pos(), "RESOURCE_EXHAUSTION", "Maximum call stack size exceeded");
                } else {
                    let mut n = self.compose_collection(ctx, token, props);
                    if let Some(a) = anchor {
                        n.anchor = Some(a.source[1..].to_string());
                    }
                    node = Some(n);
                }
                ctx.depth -= 1;
            }
            _ => {
                let msg = if token.ty == T::Error {
                    token.message.clone()
                } else {
                    format!("Unsupported token (type: {})", token.ty.name())
                };
                self.err(token.pos(), "UNEXPECTED_TOKEN", msg);
            }
        }
        let node = match node {
            Some(n) => n,
            None => self.compose_empty_node(ctx, token.offset, None, props),
        };
        if let Some(a) = anchor {
            if node.anchor.as_deref() == Some("") {
                self.err(a.pos(), "BAD_ALIAS", "Anchor cannot be an empty string");
            }
        }
        node
    }

    fn compose_empty_node<'a>(
        &mut self,
        ctx: &mut Ctx<'_>,
        offset: usize,
        before: Option<&[Tok<'a>]>,
        props: &Props<'a>,
    ) -> Node {
        let token = Tok::new(T::Scalar, empty_scalar_position(offset, before), -1, "");
        let mut node = self.compose_scalar(ctx, &token, props.tag.as_ref());
        if let Some(a) = &props.anchor {
            let name = a.source[1..].to_string();
            if name.is_empty() {
                self.err(a.pos(), "BAD_ALIAS", "Anchor cannot be an empty string");
            }
            node.anchor = Some(name);
        }
        if props.has_comment {
            node.range[2] = props.end;
        }
        node
    }

    fn compose_alias(&mut self, t: &Tok<'_>) -> Node {
        let source = &t.source[1..];
        if source.is_empty() {
            self.err_at(t.offset, "BAD_ALIAS", "Alias cannot be an empty string");
        }
        let value_end = t.offset + t.source.len();
        let re = self.resolve_end(t.end.as_deref(), value_end, true);
        Node { kind: Nk::Alias(source.to_string()), anchor: None, range: [t.offset, value_end, re] }
    }

    fn resolve_end(&mut self, end: Option<&[Tok<'_>]>, mut offset: usize, req_space: bool) -> usize {
        if let Some(end) = end {
            let mut has_space = false;
            for token in end {
                match token.ty {
                    T::Space => has_space = true,
                    T::Comment => {
                        if req_space && !has_space {
                            self.err(token.pos(), "MISSING_CHAR", "Comments must be separated from other tokens by white space characters");
                        }
                    }
                    T::Newline => has_space = true,
                    _ => self.err(token.pos(), "UNEXPECTED_TOKEN", format!("Unexpected {} at node end", token.ty.name())),
                }
                offset += token.source.len();
            }
        }
        offset
    }

    fn compose_scalar<'a>(&mut self, ctx: &Ctx<'_>, token: &Tok<'a>, tag_token: Option<&PTok<'a>>) -> Node {
        let (value, range) = if token.ty == T::BlockScalar {
            self.resolve_block_scalar(ctx, token)
        } else {
            self.resolve_flow_scalar(token)
        };
        let tag_name = tag_token.and_then(|t| {
            let pos = t.pos();
            let mut errs = Vec::new();
            let name = ctx.directives.tag_name(t.source, |m| errs.push(m));
            for m in errs {
                self.err(pos, "TAG_RESOLVE_FAILED", m);
            }
            name
        });
        let sv = match tag_name.as_deref() {
            Some("!") => Sv::Str(value),
            Some(name) => resolve_tagged(&value, name),
            None if token.ty == T::Scalar => resolve_plain(value),
            None => Sv::Str(value),
        };
        Node { kind: Nk::Scalar(sv), anchor: None, range }
    }

    fn resolve_flow_scalar(&mut self, s: &Tok<'_>) -> (String, [usize; 3]) {
        let offset = s.offset;
        let source = s.source;
        let value = match s.ty {
            T::Scalar => {
                let bad = match source.as_bytes().first() {
                    Some(b'\t') => Some("a tab character".to_string()),
                    Some(b',') => Some("flow indicator character ,".to_string()),
                    Some(b'%') => Some("directive indicator character %".to_string()),
                    Some(c @ (b'|' | b'>')) => Some(format!("block scalar indicator {}", *c as char)),
                    Some(c @ (b'@' | b'`')) => Some(format!("reserved character {}", *c as char)),
                    _ => None,
                };
                if let Some(b) = bad {
                    self.err_at(offset, "BAD_SCALAR_START", format!("Plain value cannot start with {b}"));
                }
                unfold_lines(source)
            }
            T::SingleQuoted => {
                if !source.ends_with('\'') || source.len() == 1 {
                    self.err_at(offset + source.len(), "MISSING_CHAR", "Missing closing 'quote");
                }
                let inner = js_inner(source);
                unfold_lines(inner).replace("''", "'")
            }
            _ => self.double_quoted(source, offset),
        };
        let value_end = offset + source.len();
        let re = self.resolve_end(s.end.as_deref(), value_end, true);
        (value, [offset, value_end, re])
    }

    fn double_quoted(&mut self, source: &str, offset: usize) -> String {
        let b = source.as_bytes();
        let mut res: Vec<u8> = Vec::with_capacity(b.len());
        let at = |i: usize| b.get(i).copied();
        let mut i = 1usize;
        while i + 1 < b.len() {
            let ch = b[i];
            if ch == b'\r' && at(i + 1) == Some(b'\n') {
                i += 1;
                continue;
            }
            if ch == b'\n' {
                let mut fold = Vec::new();
                let mut off = i;
                let mut c = at(off + 1);
                while matches!(c, Some(b' ' | b'\t' | b'\n' | b'\r')) {
                    if c == Some(b'\r') && at(off + 2) != Some(b'\n') {
                        break;
                    }
                    if c == Some(b'\n') {
                        fold.push(b'\n');
                    }
                    off += 1;
                    c = at(off + 1);
                }
                if fold.is_empty() {
                    fold.push(b' ');
                }
                res.extend_from_slice(&fold);
                i = off;
            } else if ch == b'\\' {
                i += 1;
                let next = at(i);
                let code: Option<&str> = match next {
                    Some(b'0') => Some("\0"),
                    Some(b'a') => Some("\u{7}"),
                    Some(b'b') => Some("\u{8}"),
                    Some(b'e') => Some("\u{1b}"),
                    Some(b'f') => Some("\u{c}"),
                    Some(b'n') => Some("\n"),
                    Some(b'r') => Some("\r"),
                    Some(b't') => Some("\t"),
                    Some(b'v') => Some("\u{b}"),
                    Some(b'N') => Some("\u{85}"),
                    Some(b'_') => Some("\u{a0}"),
                    Some(b'L') => Some("\u{2028}"),
                    Some(b'P') => Some("\u{2029}"),
                    Some(b' ') => Some(" "),
                    Some(b'"') => Some("\""),
                    Some(b'/') => Some("/"),
                    Some(b'\\') => Some("\\"),
                    Some(b'\t') => Some("\t"),
                    _ => None,
                };
                if let Some(c) = code {
                    res.extend_from_slice(c.as_bytes());
                } else if next == Some(b'\n') {
                    while matches!(at(i + 1), Some(b' ' | b'\t')) {
                        i += 1;
                    }
                } else if next == Some(b'\r') && at(i + 1) == Some(b'\n') {
                    i += 1;
                    while matches!(at(i + 1), Some(b' ' | b'\t')) {
                        i += 1;
                    }
                } else if matches!(next, Some(b'x' | b'u' | b'U')) {
                    let length = match next {
                        Some(b'x') => 2,
                        Some(b'u') => 4,
                        _ => 8,
                    };
                    // `source.substr(i + 1, length)`, counted in UTF-16 units.
                    let rest = source.get(i + 1..).unwrap_or("");
                    let (mut units, mut hex_end) = (0, 0);
                    for c in rest.chars() {
                        if units >= length {
                            break;
                        }
                        units += c.len_utf16();
                        hex_end += c.len_utf8();
                    }
                    let hex = &rest[..hex_end];
                    let code = Some(hex)
                        .filter(|h| units == length && h.bytes().all(|c| c.is_ascii_hexdigit()))
                        .and_then(|h| u32::from_str_radix(h, 16).ok());
                    match code.filter(|c| *c <= 0x10FFFF) {
                        Some(c) => {
                            let ch = char::from_u32(c).unwrap_or('\u{FFFD}');
                            let mut buf = [0; 4];
                            res.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        None => {
                            let raw = &source[i - 1..i + 1 + hex_end];
                            self.err_at(offset + i - 1, "BAD_DQ_ESCAPE", format!("Invalid escape sequence {raw}"));
                            res.extend_from_slice(raw.as_bytes());
                        }
                    }
                    i += hex_end;
                } else {
                    let end = source[i.min(source.len())..].chars().next().map_or(i, |c| i + c.len_utf8());
                    let raw = &source[i - 1..end.min(source.len())];
                    self.err_at(offset + i - 1, "BAD_DQ_ESCAPE", format!("Invalid escape sequence {raw}"));
                    res.extend_from_slice(raw.as_bytes());
                    i = end.saturating_sub(1).max(i);
                }
            } else if ch == b' ' || ch == b'\t' {
                let ws_start = i;
                while matches!(at(i + 1), Some(b' ' | b'\t')) {
                    i += 1;
                }
                let next = at(i + 1);
                if next != Some(b'\n') && !(next == Some(b'\r') && at(i + 2) == Some(b'\n')) {
                    res.extend_from_slice(&b[ws_start..=i]);
                }
            } else {
                res.push(ch);
            }
            i += 1;
        }
        if !source.ends_with('"') || source.len() == 1 {
            self.err_at(offset + source.len(), "MISSING_CHAR", "Missing closing \"quote");
        }
        String::from_utf8_lossy(&res).into_owned()
    }

    fn resolve_block_scalar(&mut self, ctx: &Ctx<'_>, scalar: &Tok<'_>) -> (String, [usize; 3]) {
        let start = scalar.offset;
        let header = self.block_scalar_header(scalar);
        let source = scalar.source;
        let literal = header.mode == b'|';
        let lines: Vec<(&str, &str)> = if source.is_empty() { Vec::new() } else { split_lines(source) };
        let mut chomp_start = lines.len();
        for i in (0..lines.len()).rev() {
            let c = lines[i].1;
            if c.is_empty() || c == "\r" {
                chomp_start = i;
            } else {
                break;
            }
        }
        if chomp_start == 0 {
            let value = if header.chomp == Some(b'+') && !lines.is_empty() {
                "\n".repeat(1.max(lines.len() - 1))
            } else {
                String::new()
            };
            let end = start + header.length + source.len();
            return (value, [start, end, end]);
        }
        let mut trim_indent = (scalar.indent + header.indent) as usize;
        let mut offset = scalar.offset + header.length;
        let mut content_start = 0;
        for (i, (indent, content)) in lines.iter().enumerate().take(chomp_start) {
            if content.is_empty() || *content == "\r" {
                if header.indent == 0 && indent.len() > trim_indent {
                    trim_indent = indent.len();
                }
            } else {
                if indent.len() < trim_indent {
                    self.err_at(offset + indent.len(), "MISSING_CHAR", "Block scalars with more-indented leading empty lines must use an explicit indentation indicator");
                }
                if header.indent == 0 {
                    trim_indent = indent.len();
                }
                content_start = i;
                if trim_indent == 0 && !ctx.at_root {
                    self.err_at(offset, "BAD_INDENT", "Block scalar values in collections must be indented");
                }
                break;
            }
            offset += indent.len() + content.len() + 1;
        }
        for i in (chomp_start..lines.len()).rev() {
            if lines[i].0.len() > trim_indent {
                chomp_start = i + 1;
            }
        }
        let slice_indent = |s: &'_ str| -> String { s.get(trim_indent..).unwrap_or("").to_string() };
        let mut value = String::new();
        let mut sep = "";
        let mut prev_more_indented = false;
        for line in lines.iter().take(content_start) {
            value.push_str(&slice_indent(line.0));
            value.push('\n');
        }
        for line in lines.iter().take(chomp_start).skip(content_start) {
            let (mut indent, mut content) = *line;
            offset += indent.len() + content.len() + 1;
            let crlf = content.ends_with('\r');
            if crlf {
                content = &content[..content.len() - 1];
            }
            if !content.is_empty() && indent.len() < trim_indent {
                let src = if header.indent != 0 { "explicit indentation indicator" } else { "first line" };
                self.err_at(
                    offset - content.len() - if crlf { 2 } else { 1 },
                    "BAD_INDENT",
                    format!("Block scalar lines must not be less indented than their {src}"),
                );
                indent = "";
            }
            if literal {
                value.push_str(sep);
                value.push_str(&slice_indent(indent));
                value.push_str(content);
                sep = "\n";
            } else if indent.len() > trim_indent || content.starts_with('\t') {
                if sep == " " {
                    sep = "\n";
                } else if !prev_more_indented && sep == "\n" {
                    sep = "\n\n";
                }
                value.push_str(sep);
                value.push_str(&slice_indent(indent));
                value.push_str(content);
                sep = "\n";
                prev_more_indented = true;
            } else if content.is_empty() {
                if sep == "\n" {
                    value.push('\n');
                } else {
                    sep = "\n";
                }
            } else {
                value.push_str(sep);
                value.push_str(content);
                sep = " ";
                prev_more_indented = false;
            }
        }
        match header.chomp {
            Some(b'-') => {}
            Some(b'+') => {
                for line in lines.iter().skip(chomp_start) {
                    value.push('\n');
                    value.push_str(&slice_indent(line.0));
                }
                if !value.ends_with('\n') {
                    value.push('\n');
                }
            }
            _ => value.push('\n'),
        }
        let end = start + header.length + source.len();
        (value, [start, end, end])
    }

    fn block_scalar_header(&mut self, s: &Tok<'_>) -> BlockHeader {
        let source = s.start[0].source;
        let b = source.as_bytes();
        let mode = b[0];
        let mut indent = 0i64;
        let mut chomp = None;
        let mut error: Option<usize> = None;
        for (i, &ch) in b.iter().enumerate().skip(1) {
            if chomp.is_none() && (ch == b'-' || ch == b'+') {
                chomp = Some(ch);
            } else {
                let n = if ch.is_ascii_digit() { i64::from(ch - b'0') } else { 0 };
                if indent == 0 && n != 0 {
                    indent = n;
                } else if error.is_none() {
                    error = Some(s.offset + i);
                }
            }
        }
        if let Some(e) = error {
            self.err_at(e, "UNEXPECTED_TOKEN", format!("Block scalar header includes extra characters: {source}"));
        }
        let mut has_space = false;
        let mut length = source.len();
        for token in &s.start[1..] {
            match token.ty {
                T::Space => {
                    has_space = true;
                    length += token.source.len();
                }
                T::Newline => length += token.source.len(),
                T::Comment => {
                    if !has_space {
                        self.err(token.pos(), "MISSING_CHAR", "Comments must be separated from other tokens by white space characters");
                    }
                    length += token.source.len();
                }
                T::Error => {
                    self.err(token.pos(), "UNEXPECTED_TOKEN", token.message.clone());
                    length += token.source.len();
                }
                _ => {
                    self.err(token.pos(), "UNEXPECTED_TOKEN", format!("Unexpected token in block scalar header: {}", token.ty.name()));
                    length += token.source.len();
                }
            }
        }
        BlockHeader { mode, indent, chomp, length }
    }

    fn compose_collection<'a>(&mut self, ctx: &mut Ctx<'_>, token: &Tok<'a>, props: &Props<'a>) -> Node {
        if let Some(t) = &props.tag {
            let pos = t.pos();
            let mut errs = Vec::new();
            ctx.directives.tag_name(t.source, |m| errs.push(m));
            for m in errs {
                self.err(pos, "TAG_RESOLVE_FAILED", m);
            }
        }
        if token.ty == T::BlockSeq {
            let last = match (&props.anchor, &props.tag) {
                (Some(a), Some(t)) => Some(if a.offset > t.offset { a } else { t }),
                (Some(a), None) => Some(a),
                (None, t) => t.as_ref(),
            };
            if let Some(l) = last {
                let nl = props.newline_after_prop.as_ref();
                if nl.is_none_or(|n| n.offset < l.offset) {
                    self.err(l.pos(), "MISSING_CHAR", "Missing newline after block sequence props");
                }
            }
        }
        match token.ty {
            T::BlockMap => self.resolve_block_map(ctx, token),
            T::BlockSeq => self.resolve_block_seq(ctx, token),
            _ => self.resolve_flow_collection(ctx, token),
        }
    }

    fn resolve_block_map<'a>(&mut self, ctx: &mut Ctx<'_>, bm: &Tok<'a>) -> Node {
        const START_COL: &str = "All mapping items must start at the same column";
        ctx.at_root = false;
        let mut offset = bm.offset;
        let mut items: Vec<(Node, Option<Node>)> = Vec::new();
        let mut comment_end: Option<usize> = None;
        for it in &bm.items {
            let key = it.key.as_deref();
            let sep = it.sep.as_deref();
            let value = it.value.as_deref();
            let key_props = self.resolve_props(&it.start, RpOpts {
                flow: None,
                indicator: T::ExplicitKeyInd,
                next: key.or_else(|| sep.and_then(<[_]>::first)),
                offset,
                parent_indent: bm.indent,
                start_on_newline: true,
            });
            let implicit_key = key_props.found.is_none();
            if implicit_key {
                if let Some(k) = key {
                    if k.ty == T::BlockSeq {
                        self.err_at(offset, "BLOCK_AS_IMPLICIT_KEY", "A block sequence may not be used as an implicit map key");
                    } else if k.indent != bm.indent {
                        self.err_at(offset, "BAD_INDENT", START_COL);
                    }
                }
                if key_props.anchor.is_none() && key_props.tag.is_none() && sep.is_none() {
                    comment_end = Some(key_props.end);
                    continue;
                }
                if key_props.newline_after_prop.is_some() || contains_newline(key) {
                    let pos = match key {
                        Some(k) => k.pos(),
                        None => it.start.last().expect("start").pos(),
                    };
                    self.err(pos, "MULTILINE_IMPLICIT_KEY", "Implicit keys need to be on a single line");
                }
            } else if key_props.found.as_ref().map(|f| f.indent) != Some(bm.indent) {
                self.err_at(offset, "BAD_INDENT", START_COL);
            }
            ctx.at_key = true;
            let key_start = key_props.end;
            let key_node = match key {
                Some(k) => self.compose_node(ctx, k, &key_props),
                None => self.compose_empty_node(ctx, key_start, Some(&it.start), &key_props),
            };
            ctx.at_key = false;
            if map_includes(&items, &key_node) {
                self.err_at(key_start, "DUPLICATE_KEY", "Map keys must be unique");
            }
            let value_props = self.resolve_props(sep.unwrap_or(&[]), RpOpts {
                flow: None,
                indicator: T::MapValueInd,
                next: value,
                offset: key_node.range[2],
                parent_indent: bm.indent,
                start_on_newline: key.is_none_or(|k| k.ty == T::BlockScalar),
            });
            offset = value_props.end;
            if let Some(found) = &value_props.found {
                if implicit_key {
                    if value.map(|v| v.ty) == Some(T::BlockMap) && !value_props.has_newline {
                        self.err_at(offset, "BLOCK_AS_IMPLICIT_KEY", "Nested mappings are not allowed in compact mappings");
                    }
                    if key_props.start as i64 + 1024 < found.offset as i64 {
                        self.err(
                            (key_node.range[0], key_node.range[1]),
                            "KEY_OVER_1024_CHARS",
                            "The : indicator must be at most 1024 chars after the start of an implicit block mapping key",
                        );
                    }
                }
                let value_node = match value {
                    Some(v) => self.compose_node(ctx, v, &value_props),
                    None => self.compose_empty_node(ctx, offset, sep, &value_props),
                };
                offset = value_node.range[2];
                items.push((key_node, Some(value_node)));
            } else {
                if implicit_key {
                    self.err((key_node.range[0], key_node.range[1]), "MISSING_CHAR", "Implicit map keys need to be followed by map values");
                }
                items.push((key_node, None));
            }
        }
        if let Some(ce) = comment_end {
            if ce < offset {
                self.err_at(ce, "IMPOSSIBLE", "Map comment with trailing content");
            }
        }
        Node { kind: Nk::Map(items), anchor: None, range: [bm.offset, offset, comment_end.unwrap_or(offset)] }
    }

    fn resolve_block_seq<'a>(&mut self, ctx: &mut Ctx<'_>, bs: &Tok<'a>) -> Node {
        ctx.at_root = false;
        ctx.at_key = false;
        let mut offset = bs.offset;
        let mut items = Vec::new();
        let mut comment_end = None;
        for it in &bs.items {
            let value = it.value.as_deref();
            let props = self.resolve_props(&it.start, RpOpts {
                flow: None,
                indicator: T::SeqItemInd,
                next: value,
                offset,
                parent_indent: bs.indent,
                start_on_newline: true,
            });
            if props.found.is_none() {
                if props.anchor.is_some() || props.tag.is_some() || value.is_some() {
                    if value.map(|v| v.ty) == Some(T::BlockSeq) {
                        self.err_at(props.end, "BAD_INDENT", "All sequence items must start at the same column");
                    } else {
                        self.err_at(offset, "MISSING_CHAR", "Sequence item without - indicator");
                    }
                } else {
                    comment_end = Some(props.end);
                    continue;
                }
            }
            let node = match value {
                Some(v) => self.compose_node(ctx, v, &props),
                None => self.compose_empty_node(ctx, props.end, Some(&it.start), &props),
            };
            offset = node.range[2];
            items.push(node);
        }
        Node { kind: Nk::Seq(items), anchor: None, range: [bs.offset, offset, comment_end.unwrap_or(offset)] }
    }

    fn resolve_flow_collection<'a>(&mut self, ctx: &mut Ctx<'_>, fc: &Tok<'a>) -> Node {
        const BLOCK_MSG: &str = "Block collections are not allowed within flow collections";
        let is_block = |t: Option<&Tok<'_>>| matches!(t.map(|t| t.ty), Some(T::BlockMap | T::BlockSeq));
        let start_src = fc.start[0].source;
        let is_map = start_src == "{";
        let fc_name = if is_map { "flow map" } else { "flow sequence" };
        let at_root = ctx.at_root;
        ctx.at_root = false;
        ctx.at_key = false;
        let mut offset = fc.offset + start_src.len();
        let mut seq_items: Vec<Node> = Vec::new();
        let mut map_items: Vec<(Node, Option<Node>)> = Vec::new();
        let n_items = fc.items.len();
        for (i, it) in fc.items.iter().enumerate() {
            let key = it.key.as_deref();
            let sep = it.sep.as_deref();
            let value = it.value.as_deref();
            let props = self.resolve_props(&it.start, RpOpts {
                flow: Some(fc_name),
                indicator: T::ExplicitKeyInd,
                next: key.or_else(|| sep.and_then(<[_]>::first)),
                offset,
                parent_indent: fc.indent,
                start_on_newline: false,
            });
            if props.found.is_none() {
                if props.anchor.is_none() && props.tag.is_none() && sep.is_none() && value.is_none() {
                    let leading_comma = props.comma.as_ref().filter(|_| i == 0);
                    if let Some(c) = leading_comma {
                        self.err(c.pos(), "UNEXPECTED_TOKEN", format!("Unexpected , in {fc_name}"));
                    } else if i + 1 < n_items {
                        self.err_at(props.start, "UNEXPECTED_TOKEN", format!("Unexpected empty item in {fc_name}"));
                    }
                    offset = props.end;
                    continue;
                }
                if !is_map && contains_newline(key) {
                    let k = key.expect("key");
                    self.err(k.pos(), "MULTILINE_IMPLICIT_KEY", "Implicit keys of flow sequence pairs need to be on a single line");
                }
            }
            if i == 0 {
                if let Some(c) = &props.comma {
                    self.err(c.pos(), "UNEXPECTED_TOKEN", format!("Unexpected , in {fc_name}"));
                }
            } else if props.comma.is_none() {
                self.err_at(props.start, "MISSING_CHAR", format!("Missing , between {fc_name} items"));
            }
            if !is_map && sep.is_none() && props.found.is_none() {
                let value_node = match value {
                    Some(v) => self.compose_node(ctx, v, &props),
                    None => self.compose_empty_node(ctx, props.end, sep, &props),
                };
                offset = value_node.range[2];
                if is_block(value) {
                    self.err((value_node.range[0], value_node.range[1]), "BLOCK_IN_FLOW", BLOCK_MSG);
                }
                seq_items.push(value_node);
            } else {
                ctx.at_key = true;
                let key_start = props.end;
                let key_node = match key {
                    Some(k) => self.compose_node(ctx, k, &props),
                    None => self.compose_empty_node(ctx, key_start, Some(&it.start), &props),
                };
                if is_block(key) {
                    self.err((key_node.range[0], key_node.range[1]), "BLOCK_IN_FLOW", BLOCK_MSG);
                }
                ctx.at_key = false;
                let value_props = self.resolve_props(sep.unwrap_or(&[]), RpOpts {
                    flow: Some(fc_name),
                    indicator: T::MapValueInd,
                    next: value,
                    offset: key_node.range[2],
                    parent_indent: fc.indent,
                    start_on_newline: false,
                });
                if let Some(found) = &value_props.found {
                    if !is_map && props.found.is_none() {
                        if let Some(sep) = sep {
                            for st in sep {
                                if st.offset == found.offset && st.ty == found.ty {
                                    break;
                                }
                                if st.ty == T::Newline {
                                    self.err(st.pos(), "MULTILINE_IMPLICIT_KEY", "Implicit keys of flow sequence pairs need to be on a single line");
                                    break;
                                }
                            }
                        }
                        if props.start as i64 + 1024 < found.offset as i64 {
                            self.err(found.pos(), "KEY_OVER_1024_CHARS", "The : indicator must be at most 1024 chars after the start of an implicit flow sequence key");
                        }
                    }
                } else if let Some(v) = value {
                    if v.has_source_len() && v.source.starts_with(':') {
                        self.err(v.pos(), "MISSING_CHAR", format!("Missing space after : in {fc_name}"));
                    } else {
                        self.err_at(value_props.start, "MISSING_CHAR", format!("Missing , or : between {fc_name} items"));
                    }
                }
                let value_node = match value {
                    Some(v) => Some(self.compose_node(ctx, v, &value_props)),
                    None if value_props.found.is_some() => {
                        Some(self.compose_empty_node(ctx, value_props.end, sep, &value_props))
                    }
                    None => None,
                };
                if let Some(vn) = &value_node {
                    if is_block(value) {
                        self.err((vn.range[0], vn.range[1]), "BLOCK_IN_FLOW", BLOCK_MSG);
                    }
                }
                offset = value_node.as_ref().map_or(value_props.end, |v| v.range[2]);
                if is_map {
                    if map_includes(&map_items, &key_node) {
                        self.err_at(key_start, "DUPLICATE_KEY", "Map keys must be unique");
                    }
                    map_items.push((key_node, value_node));
                } else {
                    let end_range = value_node.as_ref().unwrap_or(&key_node).range;
                    let range = [key_node.range[0], end_range[1], end_range[2]];
                    seq_items.push(Node { kind: Nk::Map(vec![(key_node, value_node)]), anchor: None, range });
                }
            }
        }
        let expected_end = if is_map { "}" } else { "]" };
        let empty: Vec<Tok<'a>> = Vec::new();
        let end_toks = fc.end.as_ref().unwrap_or(&empty);
        let ce = end_toks.first();
        let mut ee: Vec<&Tok<'a>> = end_toks.iter().skip(1).collect();
        let mut ce_pos = offset;
        if ce.map(|c| c.source) == Some(expected_end) {
            ce_pos = ce.expect("ce").offset + expected_end.len();
        } else {
            let name = if is_map { "Flow map" } else { "Flow sequence" };
            if at_root {
                self.err_at(offset, "MISSING_CHAR", format!("{name} must end with a {expected_end}"));
            } else {
                self.err_at(offset, "BAD_INDENT", format!("{name} in block collection must be sufficiently indented and end with a {expected_end}"));
            }
            if let Some(c) = ce {
                if c.source.len() != 1 {
                    ee.insert(0, c);
                }
            }
        }
        let range = if ee.is_empty() {
            [fc.offset, ce_pos, ce_pos]
        } else {
            let owned: Vec<Tok<'a>> = ee.into_iter().cloned().collect();
            let end = self.resolve_end(Some(&owned), ce_pos, true);
            [fc.offset, ce_pos, end]
        };
        let kind = if is_map { Nk::Map(map_items) } else { Nk::Seq(seq_items) };
        Node { kind, anchor: None, range }
    }
}

struct BlockHeader {
    mode: u8,
    indent: i64,
    chomp: Option<u8>,
    length: usize,
}

/// `source.slice(1, -1)`.
fn js_inner(source: &str) -> &str {
    if source.len() < 2 {
        return "";
    }
    let mut end = source.len() - 1;
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    &source[1..end.max(1)]
}

fn split_lines(source: &str) -> Vec<(&str, &str)> {
    // `source.split(/\n( *)/)`, as `[indent, content]` pairs.
    let b = source.as_bytes();
    let first_len = b.iter().take_while(|c| **c == b' ').count();
    let mut lines = Vec::new();
    let mut indent = &source[..first_len];
    let mut start = first_len;
    let mut i = first_len;
    while i < b.len() {
        if b[i] == b'\n' {
            lines.push((indent, &source[start..i]));
            let mut j = i + 1;
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            indent = &source[i + 1..j];
            start = j;
            i = j;
        } else {
            i += 1;
        }
    }
    lines.push((indent, &source[start..]));
    lines
}

fn trim_ws_end(s: &str) -> &str {
    s.trim_end_matches([' ', '\t'])
}

fn unfold_lines(source: &str) -> String {
    let Some(first_nl) = source.find('\n') else {
        return source.to_string();
    };
    let line_of = |s: &'_ str| -> usize {
        if s.ends_with('\r') {
            s.len() - 1
        } else {
            s.len()
        }
    };
    let first = &source[..first_nl];
    let first = &first[..line_of(first)];
    let mut res = trim_ws_end(first).to_string();
    let mut sep = " ";
    let mut pos = first_nl + 1;
    while let Some(p) = source[pos..].find('\n') {
        let l = &source[pos..pos + p];
        let l = &l[..line_of(l)];
        let lm = trim_ws_end(l.trim_start_matches([' ', '\t']));
        if lm.is_empty() {
            if sep == "\n" {
                res.push_str(sep);
            } else {
                sep = "\n";
            }
        } else {
            res.push_str(sep);
            res.push_str(lm);
            sep = " ";
        }
        pos += p + 1;
    }
    res.push_str(sep);
    res.push_str(source[pos..].trim_start_matches([' ', '\t']));
    res
}

fn empty_scalar_position(mut offset: usize, before: Option<&[Tok<'_>]>) -> usize {
    if let Some(before) = before {
        let mut i = before.len() as i64 - 1;
        while i >= 0 {
            let st = &before[i as usize];
            if matches!(st.ty, T::Space | T::Comment | T::Newline) {
                offset -= st.source.len();
                i -= 1;
                continue;
            }
            let mut k = i as usize + 1;
            while let Some(st) = before.get(k).filter(|t| t.ty == T::Space) {
                offset += st.source.len();
                k += 1;
            }
            break;
        }
    }
    offset
}

fn contains_newline(key: Option<&Tok<'_>>) -> bool {
    let Some(key) = key else { return false };
    match key.ty {
        T::Alias | T::Scalar | T::DoubleQuoted | T::SingleQuoted => {
            key.source.contains('\n')
                || key.end.as_ref().is_some_and(|e| e.iter().any(|t| t.ty == T::Newline))
        }
        T::FlowCollection => key.items.iter().any(|it| {
            it.start.iter().any(|t| t.ty == T::Newline)
                || it.sep.as_ref().is_some_and(|s| s.iter().any(|t| t.ty == T::Newline))
                || contains_newline(it.key.as_deref())
                || contains_newline(it.value.as_deref())
        }),
        _ => true,
    }
}

fn map_includes(items: &[(Node, Option<Node>)], search: &Node) -> bool {
    let Nk::Scalar(s) = &search.kind else { return false };
    items.iter().any(|(k, _)| {
        let Nk::Scalar(a) = &k.kind else { return false };
        match (a, s) {
            (Sv::Null, Sv::Null) => true,
            (Sv::Bool(x), Sv::Bool(y)) => x == y,
            (Sv::Num(x), Sv::Num(y)) => x == y,
            (Sv::Str(x), Sv::Str(y)) => x == y,
            _ => false,
        }
    })
}

// ---- core schema -----------------------------------------------------------

fn test_null(s: &str) -> bool {
    matches!(s, "" | "~" | "null" | "Null" | "NULL")
}
fn test_bool(s: &str) -> bool {
    matches!(s, "true" | "True" | "TRUE" | "false" | "False" | "FALSE")
}
fn test_oct(s: &str) -> bool {
    s.strip_prefix("0o").is_some_and(|r| !r.is_empty() && r.bytes().all(|c| (b'0'..=b'7').contains(&c)))
}
fn strip_sign(s: &str) -> &str {
    s.strip_prefix(['-', '+']).unwrap_or(s)
}
fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit())
}
fn test_int(s: &str) -> bool {
    all_digits(strip_sign(s))
}
fn test_hex(s: &str) -> bool {
    s.strip_prefix("0x").is_some_and(|r| !r.is_empty() && r.bytes().all(|c| c.is_ascii_hexdigit()))
}
fn test_float_nan(s: &str) -> bool {
    matches!(strip_sign(s), ".inf" | ".Inf" | ".INF") || matches!(s, ".nan" | ".NaN" | ".NAN")
}
fn test_float_exp(s: &str) -> bool {
    let r = strip_sign(s);
    let Some(e) = r.find(['e', 'E']) else { return false };
    let (m, x) = (&r[..e], &r[e + 1..]);
    let m_ok = match m.split_once('.') {
        Some(("", f)) => all_digits(f),
        Some((i, f)) => all_digits(i) && (f.is_empty() || all_digits(f)),
        None => all_digits(m),
    };
    m_ok && all_digits(strip_sign(x))
}
fn test_float(s: &str) -> bool {
    match strip_sign(s).split_once('.') {
        Some(("", f)) => all_digits(f),
        Some((i, f)) => all_digits(i) && (f.is_empty() || all_digits(f)),
        None => false,
    }
}

fn parse_radix(s: &str, radix: u32) -> f64 {
    let mut v = 0f64;
    for c in s.chars() {
        v = v * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0));
    }
    v
}

fn res_int(s: &str) -> f64 {
    s.parse::<f64>().unwrap_or(f64::NAN)
}

fn res_float_nan(s: &str) -> f64 {
    if s.ends_with("nan") || s.ends_with("NaN") || s.ends_with("NAN") {
        f64::NAN
    } else if s.starts_with('-') {
        f64::NEG_INFINITY
    } else {
        f64::INFINITY
    }
}

fn is_true(s: &str) -> bool {
    s.starts_with(['t', 'T'])
}

fn resolve_plain(value: String) -> Sv {
    let s = value.as_str();
    if test_null(s) {
        Sv::Null
    } else if test_bool(s) {
        Sv::Bool(is_true(s))
    } else if test_oct(s) {
        Sv::Num(parse_radix(&s[2..], 8))
    } else if test_int(s) {
        Sv::Num(res_int(s))
    } else if test_hex(s) {
        Sv::Num(parse_radix(&s[2..], 16))
    } else if test_float_nan(s) {
        Sv::Num(res_float_nan(s))
    } else if test_float_exp(s) || test_float(s) {
        Sv::Num(s.parse::<f64>().unwrap_or(f64::NAN))
    } else {
        Sv::Str(value)
    }
}

/// `findScalarTagByName` then `resolve`, core schema only. An unknown tag is a
/// warning upstream and resolves to a string.
fn resolve_tagged(value: &str, name: &str) -> Sv {
    let s = value;
    match name {
        "tag:yaml.org,2002:str" => Sv::Str(s.to_string()),
        "tag:yaml.org,2002:null" if test_null(s) => Sv::Null,
        "tag:yaml.org,2002:bool" if test_bool(s) => Sv::Bool(is_true(s)),
        "tag:yaml.org,2002:int" if test_oct(s) => Sv::Num(parse_radix(&s[2..], 8)),
        "tag:yaml.org,2002:int" if test_int(s) => Sv::Num(res_int(s)),
        "tag:yaml.org,2002:int" if test_hex(s) => Sv::Num(parse_radix(&s[2..], 16)),
        "tag:yaml.org,2002:float" if test_float_nan(s) => Sv::Num(res_float_nan(s)),
        "tag:yaml.org,2002:float" if test_float_exp(s) || test_float(s) => {
            Sv::Num(s.parse::<f64>().unwrap_or(f64::NAN))
        }
        _ => Sv::Str(s.to_string()),
    }
}

// ---- top level -------------------------------------------------------------

struct Doc {
    contents: Option<Node>,
    errors: Vec<ParseErr>,
    range: [usize; 3],
}

fn compose_doc<'a>(c: &mut Composer, dirs: &Directives, token: &Tok<'a>) -> Doc {
    let mut ctx = Ctx { at_key: false, at_root: true, directives: dirs, depth: 0 };
    let value = token.value.as_deref();
    let props = c.resolve_props(&token.start, RpOpts {
        flow: None,
        indicator: T::DocStart,
        next: value.or_else(|| token.end.as_ref().and_then(|e| e.first())),
        offset: token.offset,
        parent_indent: 0,
        start_on_newline: true,
    });
    if props.found.is_some()
        && matches!(value.map(|v| v.ty), Some(T::BlockMap | T::BlockSeq))
        && !props.has_newline
    {
        c.err_at(props.end, "MISSING_CHAR", "Block collection cannot start on same line with directives-end marker");
    }
    let contents = match value {
        Some(v) => c.compose_node(&mut ctx, v, &props),
        None => c.compose_empty_node(&mut ctx, props.end, Some(&token.start), &props),
    };
    let content_end = contents.range[2];
    let re = c.resolve_end(token.end.as_deref(), content_end, false);
    Doc { contents: Some(contents), errors: Vec::new(), range: [token.offset, content_end, re] }
}

fn doc_for(src: &str) -> Doc {
    let (tokens, line_starts) = Parser::parse(src);
    let mut c = Composer { errors: Vec::new() };
    let mut dirs = Directives::new();
    let mut at_directives = false;
    let mut current: Option<Doc> = None;
    let mut first: Option<Doc> = None;
    let mut done = false;
    let yield_doc = |d: Doc, first: &mut Option<Doc>, done: &mut bool| match first {
        None => *first = Some(d),
        Some(f) => {
            f.errors.push(ParseErr {
                pos: (d.range[0], d.range[1]),
                code: "MULTIPLE_DOCS",
                message: "Source contains multiple documents; please use YAML.parseAllDocuments()".into(),
            });
            *done = true;
        }
    };
    for token in &tokens {
        match token.ty {
            T::Directive => {
                let pos = token.pos();
                let mut msgs = Vec::new();
                dirs.add(token.source, |off, m| msgs.push((off, m)));
                for (off, m) in msgs {
                    c.err((pos.0 + off, pos.1), "BAD_DIRECTIVE", m);
                }
                at_directives = true;
            }
            T::Document => {
                let doc_dirs = dirs.at_document();
                let mut doc = compose_doc(&mut c, &doc_dirs, token);
                let doc_start = token.start.iter().any(|t| t.ty == T::DocStart);
                if at_directives && !doc_start {
                    c.err(token.pos(), "MISSING_CHAR", "Missing directives-end/doc-start indicator line");
                }
                doc.errors = std::mem::take(&mut c.errors);
                if let Some(d) = current.take() {
                    yield_doc(d, &mut first, &mut done);
                    if done {
                        break;
                    }
                }
                current = Some(doc);
                at_directives = false;
            }
            T::Bom | T::Space | T::Comment | T::Newline => {}
            T::Error => {
                let msg = if token.source.is_empty() {
                    token.message.clone()
                } else {
                    format!("{}: {}", token.message, js::quote(token.source))
                };
                let e = ParseErr { pos: token.pos(), code: "UNEXPECTED_TOKEN", message: msg };
                match &mut current {
                    Some(d) if !at_directives => d.errors.push(e),
                    _ => c.errors.push(e),
                }
            }
            T::DocEnd => {
                let Some(d) = &mut current else {
                    c.err(token.pos(), "UNEXPECTED_TOKEN", "Unexpected doc-end without preceding document");
                    continue;
                };
                let end = c.resolve_end(token.end.as_deref(), token.offset + token.source.len(), true);
                d.errors.append(&mut c.errors);
                d.range[2] = end;
            }
            _ => c.err(token.pos(), "UNEXPECTED_TOKEN", format!("Unsupported token {}", token.ty.name())),
        }
    }
    if !done {
        if let Some(mut d) = current.take() {
            d.errors.append(&mut c.errors);
            yield_doc(d, &mut first, &mut done);
        } else if first.is_none() {
            if at_directives {
                c.err_at(src.len(), "MISSING_CHAR", "Missing directives-end indicator line");
            }
            first = Some(Doc {
                contents: None,
                errors: std::mem::take(&mut c.errors),
                range: [0, src.len(), src.len()],
            });
        }
    }
    let mut doc = first.expect("a document");
    if let Some(e) = doc.errors.first_mut() {
        e.message = prettify(src, &line_starts, e);
    }
    doc
}

fn line_pos(src: &str, line_starts: &[usize], offset: usize) -> (usize, usize) {
    let (mut low, mut high) = (0, line_starts.len());
    while low < high {
        let mid = (low + high) >> 1;
        if line_starts[mid] < offset {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    if line_starts.get(low) == Some(&offset) {
        return (low + 1, 1);
    }
    if low == 0 {
        return (0, offset);
    }
    let start = line_starts[low - 1];
    let mut end = offset.min(src.len());
    while !src.is_char_boundary(end) {
        end -= 1;
    }
    let over = offset - end;
    (low, utf16_len(&src[start.min(end)..end]) + over + 1)
}

fn prettify(src: &str, line_starts: &[usize], e: &ParseErr) -> String {
    let (line, col) = line_pos(src, line_starts, e.pos.0);
    let mut msg = format!("{} at line {line}, column {col}", e.message);
    let mut ci = col.saturating_sub(1);
    let ls = |n: usize| line_starts.get(n).copied().unwrap_or(src.len());
    if line == 0 {
        return msg;
    }
    let units = |s: &str| -> Vec<u16> { s.encode_utf16().collect() };
    let mut line_str: Vec<u16> = {
        let s = src.get(ls(line - 1)..ls(line)).unwrap_or("");
        units(s.trim_end_matches(['\n', '\r']))
    };
    let ellipsis = 0x2026u16;
    if ci >= 60 && line_str.len() > 80 {
        let trim_start = (ci - 39).min(line_str.len() - 79);
        let mut v = vec![ellipsis];
        v.extend_from_slice(&line_str[trim_start..]);
        line_str = v;
        ci -= trim_start - 1;
    }
    if line_str.len() > 80 {
        line_str.truncate(79);
        line_str.push(ellipsis);
    }
    let prefix = &line_str[..ci.min(line_str.len())];
    if line > 1 && prefix.iter().all(|c| *c == 0x20) {
        let mut prev = units(src.get(ls(line - 2)..ls(line - 1)).unwrap_or(""));
        if prev.len() > 80 {
            prev.truncate(79);
            prev.push(ellipsis);
            prev.push(0x0A);
        }
        prev.extend_from_slice(&line_str);
        line_str = prev;
    }
    if line_str.iter().any(|c| *c != 0x20) {
        let mut count = 1i64;
        let (eline, ecol) = line_pos(src, line_starts, e.pos.1);
        if eline == line && ecol > col {
            count = 1.max(((ecol - col) as i64).min(80 - ci as i64));
        }
        let pointer = format!("{}{}", " ".repeat(ci), "^".repeat(count as usize));
        msg.push_str(&format!(":\n\n{}\n{pointer}\n", String::from_utf16_lossy(&line_str)));
    }
    msg
}

// ---- toJS ------------------------------------------------------------------

struct AnchorData {
    res: Value,
    count: f64,
    alias_count: f64,
}

struct JsCtx<'n> {
    root: &'n Node,
    /// Alias and anchored nodes in document order, built on first use.
    nodes: Vec<&'n Node>,
    built: bool,
    anchors: HashMap<*const Node, AnchorData>,
}

fn reference_error(message: String) -> YamlError {
    YamlError { name: "ReferenceError".into(), message, code: None }
}

fn collect_nodes<'n>(n: &'n Node, out: &mut Vec<&'n Node>) {
    let has_anchor = n.anchor.as_deref().is_some_and(|a| !a.is_empty());
    if matches!(n.kind, Nk::Alias(_)) || has_anchor {
        out.push(n);
    }
    match &n.kind {
        Nk::Map(items) => {
            for (k, v) in items {
                collect_nodes(k, out);
                if let Some(v) = v {
                    collect_nodes(v, out);
                }
            }
        }
        Nk::Seq(items) => {
            for i in items {
                collect_nodes(i, out);
            }
        }
        _ => {}
    }
}

impl<'n> JsCtx<'n> {
    fn find_source(&mut self, alias: &'n Node, name: &str) -> Option<&'n Node> {
        if !self.built {
            let mut nodes = Vec::new();
            collect_nodes(self.root, &mut nodes);
            self.nodes = nodes;
            self.built = true;
        }
        let mut found = None;
        for n in &self.nodes {
            if std::ptr::eq(*n, alias) {
                break;
            }
            if n.anchor.as_deref() == Some(name) {
                found = Some(*n);
            }
        }
        found
    }

    fn alias_count(&mut self, node: &'n Node) -> f64 {
        match &node.kind {
            Nk::Alias(name) => {
                let source = self.find_source(node, name);
                match source.and_then(|s| self.anchors.get(&(s as *const Node))) {
                    Some(a) => a.count * a.alias_count,
                    None => 0.0,
                }
            }
            Nk::Map(items) => {
                let mut count = 0f64;
                for (k, v) in items {
                    let kc = self.alias_count(k);
                    let vc = v.as_ref().map_or(1.0, |v| self.alias_count(v));
                    count = count.max(kc.max(vc));
                }
                count
            }
            Nk::Seq(items) => {
                let mut count = 0f64;
                for i in items {
                    count = count.max(self.alias_count(i));
                }
                count
            }
            Nk::Scalar(_) => 1.0,
        }
    }

    fn js_value(&mut self, node: &'n Node) -> Result<Value, YamlError> {
        let has_anchor = node.anchor.as_deref().is_some_and(|a| !a.is_empty());
        if has_anchor {
            let placeholder = match node.kind {
                Nk::Map(_) => Value::Object(Object::new()),
                Nk::Seq(_) => Value::Array(Vec::new()),
                _ => Value::Null,
            };
            self.anchors.insert(node, AnchorData { res: placeholder, count: 1.0, alias_count: 0.0 });
        }
        let res = self.node_to_js(node)?;
        if has_anchor {
            self.anchors.get_mut(&(node as *const Node)).expect("anchor").res = res.clone();
        }
        Ok(res)
    }

    fn node_to_js(&mut self, node: &'n Node) -> Result<Value, YamlError> {
        Ok(match &node.kind {
            Nk::Scalar(sv) => match sv {
                Sv::Null => Value::Null,
                Sv::Bool(b) => Value::Bool(*b),
                Sv::Num(n) => Value::Number(*n),
                Sv::Str(s) => Value::String(s.clone()),
            },
            Nk::Seq(items) => {
                let mut out = Vec::with_capacity(items.len());
                for i in items {
                    out.push(self.js_value(i)?);
                }
                Value::Array(out)
            }
            Nk::Map(items) => {
                let mut o = Object::new();
                for (k, v) in items {
                    let js_key = self.js_value(k)?;
                    let key = string_key(&js_key);
                    let val = match v {
                        Some(v) => self.js_value(v)?,
                        None => Value::Null,
                    };
                    o.insert(key, val);
                }
                Value::Object(o)
            }
            Nk::Alias(name) => {
                let Some(source) = self.find_source(node, name) else {
                    return Err(reference_error(format!(
                        "Unresolved alias (the anchor must be set before the alias): {name}"
                    )));
                };
                let key: *const Node = source;
                if !self.anchors.contains_key(&key) {
                    self.js_value(source)?;
                }
                let need = self.anchors.get(&key).is_some_and(|d| d.alias_count == 0.0);
                if need {
                    let c = self.alias_count(source);
                    self.anchors.get_mut(&key).expect("anchor").alias_count = c;
                }
                let data = self.anchors.get_mut(&key).expect("anchor");
                data.count += 1.0;
                if data.count * data.alias_count > 100.0 {
                    return Err(reference_error(
                        "Excessive alias count indicates a resource exhaustion attack".into(),
                    ));
                }
                data.res.clone()
            }
        })
    }
}

fn string_key(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => js::number_to_string(*n),
        Value::String(s) => s.clone(),
        other => {
            let mut ctx = SCtx::root();
            ctx.in_flow = Some(true);
            stringify_item(other, &ctx)
        }
    }
}

/// `YAML.parse(src)`.
pub fn parse(src: &str) -> Result<Value, YamlError> {
    let doc = doc_for(src);
    if let Some(e) = doc.errors.into_iter().next() {
        return Err(YamlError { name: "YAMLParseError".into(), message: e.message, code: Some(e.code) });
    }
    match &doc.contents {
        None => Ok(Value::Null),
        Some(n) => {
            let mut ctx = JsCtx { root: n, nodes: Vec::new(), built: false, anchors: HashMap::new() };
            ctx.js_value(n)
        }
    }
}

// ---------------------------------------------------------------------------
// stringify
// ---------------------------------------------------------------------------

const LINE_WIDTH: usize = 80;
const MIN_CONTENT_WIDTH: usize = 20;
const INDENT_STEP: &str = "  ";

#[derive(Clone)]
struct SCtx {
    indent: String,
    indent_at_start: Option<usize>,
    implicit_key: bool,
    in_flow: Option<bool>,
    force_block_indent: bool,
}

impl SCtx {
    fn root() -> SCtx {
        SCtx { indent: String::new(), indent_at_start: None, implicit_key: false, in_flow: None, force_block_indent: false }
    }
}

/// `YAML.stringify(value)`.
pub fn stringify(value: &Value) -> String {
    let mut s = stringify_item(value, &SCtx::root());
    s.push('\n');
    s
}

fn stringify_item(v: &Value, ctx: &SCtx) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => stringify_number(*n),
        Value::String(s) => stringify_string(s, ctx),
        Value::Array(items) => stringify_seq(items, ctx),
        Value::Object(o) => stringify_map(o, ctx),
    }
}

fn stringify_number(n: f64) -> String {
    if n.is_nan() {
        ".nan".into()
    } else if n.is_infinite() {
        if n < 0.0 { "-.inf".into() } else { ".inf".into() }
    } else if n == 0.0 && n.is_sign_negative() {
        "-0".into()
    } else {
        js::number_to_string(n)
    }
}

fn stringify_seq(items: &[Value], ctx: &SCtx) -> String {
    if ctx.in_flow == Some(true) {
        let lines = items
            .iter()
            .map(|i| {
                let mut c = ctx.clone();
                c.indent = format!("{}{}{}", ctx.indent, INDENT_STEP, INDENT_STEP);
                stringify_item(i, &c)
            })
            .collect();
        return flow_collection(lines, ctx, '[', ']');
    }
    if items.is_empty() {
        return "[]".into();
    }
    let item_indent = format!("{}{}", ctx.indent, INDENT_STEP);
    let mut item_ctx = ctx.clone();
    item_ctx.indent = item_indent;
    let mut out = String::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push('\n');
            out.push_str(&ctx.indent);
        }
        out.push_str("- ");
        out.push_str(&stringify_item(item, &item_ctx));
    }
    out
}

fn stringify_map(o: &Object, ctx: &SCtx) -> String {
    if ctx.in_flow == Some(true) {
        let lines = o
            .iter()
            .map(|(k, v)| {
                let mut c = ctx.clone();
                c.indent = format!("{}{}", ctx.indent, INDENT_STEP);
                stringify_pair(k, v, &c)
            })
            .collect();
        return flow_collection(lines, ctx, '{', '}');
    }
    if o.is_empty() {
        return "{}".into();
    }
    let mut out = String::new();
    for (i, (k, v)) in o.iter().enumerate() {
        if i > 0 {
            out.push('\n');
            out.push_str(&ctx.indent);
        }
        out.push_str(&stringify_pair(k, v, ctx));
    }
    out
}

fn flow_collection(mut lines: Vec<String>, ctx: &SCtx, open: char, close: char) -> String {
    if lines.is_empty() {
        return format!("{open}{close}");
    }
    let last = lines.len() - 1;
    let mut req_newline = false;
    for (i, l) in lines.iter_mut().enumerate() {
        req_newline = req_newline || l.contains('\n');
        if i < last {
            l.push(',');
        }
    }
    if !req_newline {
        let len: usize = lines.iter().map(|l| utf16_len(l) + 2).sum::<usize>() + 2;
        req_newline = len > LINE_WIDTH;
    }
    if req_newline {
        let mut s = String::new();
        s.push(open);
        for l in &lines {
            s.push('\n');
            s.push_str(INDENT_STEP);
            s.push_str(&ctx.indent);
            s.push_str(l);
        }
        s.push('\n');
        s.push_str(&ctx.indent);
        s.push(close);
        s
    } else {
        format!("{open} {} {close}", lines.join(" "))
    }
}

fn stringify_pair(key: &str, value: &Value, ctx: &SCtx) -> String {
    let indent = ctx.indent.clone();
    let mut pair_ctx = ctx.clone();
    pair_ctx.implicit_key = true;
    pair_ctx.indent = format!("{indent}{INDENT_STEP}");
    let mut str_ = stringify_string(key, &pair_ctx);
    let in_flow = pair_ctx.in_flow == Some(true);
    let explicit_key = !in_flow && utf16_len(&str_) > 1024;
    if explicit_key {
        str_ = format!("? {str_}\n{indent}:");
    } else {
        str_.push(':');
    }
    pair_ctx.implicit_key = false;
    let is_scalar = !matches!(value, Value::Array(_) | Value::Object(_));
    if !explicit_key && is_scalar {
        pair_ctx.indent_at_start = Some(utf16_len(&str_) + 1);
    }
    let value_str = stringify_item(value, &pair_ctx);
    let mut ws = " ".to_string();
    if !explicit_key && !is_scalar {
        let nl = value_str.contains('\n');
        let empty = match value {
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            _ => false,
        };
        let flow = pair_ctx.in_flow.unwrap_or(empty);
        if nl || !flow {
            ws = format!("\n{}", pair_ctx.indent);
        }
    } else if value_str.is_empty() || value_str.starts_with('\n') {
        ws = String::new();
    }
    str_.push_str(&ws);
    str_.push_str(&value_str);
    str_
}

fn contains_document_marker(s: &str) -> bool {
    let at_start = |l: &str| l.starts_with('%') || l.starts_with("---") || l.starts_with("...");
    if at_start(s) {
        return true;
    }
    for (i, c) in s.char_indices() {
        if matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') && at_start(&s[i + c.len_utf8()..]) {
            return true;
        }
    }
    false
}

/// `s.replace(/\n+/g, r)`, where `$&` in `r` is the matched run.
fn replace_newline_runs(s: &str, before: &str, after: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '\n' {
            let mut j = i + 1;
            while let Some((k, '\n')) = chars.peek().copied() {
                chars.next();
                j = k + 1;
            }
            out.push_str(before);
            out.push_str(&s[i..j]);
            out.push_str(after);
        } else {
            out.push(c);
        }
    }
    out
}

fn fold_options(ctx: &SCtx, is_block: bool) -> Option<usize> {
    if is_block {
        Some(utf16_len(&ctx.indent))
    } else {
        ctx.indent_at_start
    }
}

fn stringify_string(value: &str, ctx: &SCtx) -> String {
    let control = value.chars().any(|c| matches!(c, '\u{0}'..='\u{8}' | '\u{b}'..='\u{1f}' | '\u{7f}'..='\u{9f}'));
    if control {
        double_quoted_string(value, ctx)
    } else {
        plain_string(value, ctx)
    }
}

fn quoted_string(value: &str, ctx: &SCtx) -> String {
    let has_double = value.contains('"');
    let has_single = value.contains('\'');
    if has_double && !has_single {
        single_quoted_string(value, ctx)
    } else {
        double_quoted_string(value, ctx)
    }
}

fn single_quoted_string(value: &str, ctx: &SCtx) -> String {
    if (ctx.implicit_key && value.contains('\n'))
        || value.contains(" \n")
        || value.contains("\t\n")
        || value.contains("\n ")
        || value.contains("\n\t")
    {
        return double_quoted_string(value, ctx);
    }
    let indent = if !ctx.indent.is_empty() {
        ctx.indent.as_str()
    } else if contains_document_marker(value) {
        "  "
    } else {
        ""
    };
    let body = replace_newline_runs(&value.replace('\'', "''"), "", &format!("\n{indent}"));
    let res = format!("'{body}'");
    if ctx.implicit_key {
        res
    } else {
        fold_flow_lines(&res, indent, Fold::Flow, fold_options(ctx, false)).0
    }
}

fn double_quoted_string(value: &str, ctx: &SCtx) -> String {
    let json = js::quote(value);
    let indent = if !ctx.indent.is_empty() {
        ctx.indent.as_str()
    } else if contains_document_marker(value) {
        "  "
    } else {
        ""
    };
    let j = json.as_bytes();
    let at = |i: usize| j.get(i).copied();
    let mut s = String::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < j.len() {
        let mut ch = j[i];
        if ch == b' ' && at(i + 1) == Some(b'\\') && at(i + 2) == Some(b'n') {
            s.push_str(&json[start..i]);
            s.push_str("\\ ");
            i += 1;
            start = i;
            ch = b'\\';
        }
        if ch == b'\\' {
            match at(i + 1) {
                Some(b'u') => {
                    s.push_str(&json[start..i]);
                    let code = &json[i + 2..i + 6];
                    match code {
                        "0000" => s.push_str("\\0"),
                        "0007" => s.push_str("\\a"),
                        "000b" => s.push_str("\\v"),
                        "001b" => s.push_str("\\e"),
                        "0085" => s.push_str("\\N"),
                        "00a0" => s.push_str("\\_"),
                        "2028" => s.push_str("\\L"),
                        "2029" => s.push_str("\\P"),
                        _ => {
                            if let Some(low) = code.strip_prefix("00") {
                                s.push_str("\\x");
                                s.push_str(low);
                            } else {
                                s.push_str(&json[i..i + 6]);
                            }
                        }
                    }
                    i += 5;
                    start = i + 1;
                }
                Some(b'n') => {
                    if ctx.implicit_key || at(i + 2) == Some(b'"') || utf16_len(&json) < 40 {
                        i += 1;
                    } else {
                        s.push_str(&json[start..i]);
                        s.push_str("\n\n");
                        while at(i + 2) == Some(b'\\') && at(i + 3) == Some(b'n') && at(i + 4) != Some(b'"') {
                            s.push('\n');
                            i += 2;
                        }
                        s.push_str(indent);
                        if at(i + 2) == Some(b' ') {
                            s.push('\\');
                        }
                        i += 1;
                        start = i + 1;
                    }
                }
                _ => i += 1,
            }
        }
        i += 1;
    }
    let out = if start > 0 {
        s.push_str(&json[start.min(json.len())..]);
        s
    } else {
        json.clone()
    };
    if ctx.implicit_key {
        out
    } else {
        fold_flow_lines(&out, indent, Fold::Quoted, fold_options(ctx, false)).0
    }
}

fn line_length_over_limit(s: &[u16], line_width: usize, indent_len: usize) -> bool {
    let limit = line_width as i64 - indent_len as i64;
    let len = s.len() as i64;
    if len <= limit {
        return false;
    }
    let mut start = 0i64;
    for (i, c) in s.iter().enumerate() {
        if *c == 0x0A {
            if i as i64 - start > limit {
                return true;
            }
            start = i as i64 + 1;
            if len - start <= limit {
                return false;
            }
        }
    }
    true
}

fn js_replace_regex(re: &str, s: &str, repl: impl Fn(&lq_json::regex::Match, &str) -> String) -> String {
    let re = JsRegex::new(re, "").expect("static regex");
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for m in re.find_iter(s) {
        out.push_str(&s[last..m.start()]);
        out.push_str(&repl(&m, s));
        last = m.end();
    }
    out.push_str(&s[last..]);
    out
}

fn block_string(value: &str, ctx: &SCtx) -> String {
    let ends_ws_after_nl = {
        let t = value.trim_end_matches(['\t', ' ']);
        t.len() != value.len() && t.ends_with('\n')
    };
    if ends_ws_after_nl {
        return quoted_string(value, ctx);
    }
    let indent: &str = if !ctx.indent.is_empty() {
        &ctx.indent
    } else if ctx.force_block_indent || contains_document_marker(value) {
        "  "
    } else {
        ""
    };
    let units: Vec<u16> = value.encode_utf16().collect();
    let literal = !line_length_over_limit(&units, LINE_WIDTH, utf16_len(indent));
    if value.is_empty() {
        return if literal { "|\n".into() } else { ">\n".into() };
    }
    let b = value.as_bytes();
    let mut end_start = b.len();
    while end_start > 0 && matches!(b[end_start - 1], b'\n' | b'\t' | b' ') {
        end_start -= 1;
    }
    let end_full = &value[end_start..];
    let end_nl = end_full.find('\n');
    let chomp = match end_nl {
        None => "-",
        Some(p) if value == end_full || p != end_full.len() - 1 => "+",
        _ => "",
    };
    let mut body = &value[..end_start];
    let mut end = end_full.to_string();
    if !end.is_empty() {
        if end.ends_with('\n') {
            end.pop();
        }
        let ind = indent.to_string();
        end = js_replace_regex("(^|(?<!\n))\n+(?!\n|$)", &end, |m, s| format!("{}{ind}", m.as_str(s)));
    }
    let bb = body.as_bytes();
    let mut start_with_space = false;
    let mut start_end = 0;
    let mut start_nl: i64 = -1;
    while start_end < bb.len() {
        match bb[start_end] {
            b' ' => start_with_space = true,
            b'\n' => start_nl = start_end as i64,
            _ => break,
        }
        start_end += 1;
    }
    let start_len = if start_nl < start_end as i64 { (start_nl + 1) as usize } else { start_end };
    let mut start = body[..start_len].to_string();
    if !start.is_empty() {
        body = &body[start_len..];
        start = replace_newline_runs(&start, "", indent);
    }
    let indent_size = if indent.is_empty() { "1" } else { "2" };
    let header = format!("{}{chomp}", if start_with_space { indent_size } else { "" });
    if !literal {
        let folded = replace_newline_runs(body, "\n", "");
        let folded = js_replace_regex(
            "(?:^|\n)([\t ].*)(?:([\n\t ]*)\n(?![\n\t ]))?",
            &folded,
            |m, s| format!("{}{}", m.group(s, 1).unwrap_or(""), m.group(s, 2).unwrap_or("")),
        );
        let folded = replace_newline_runs(&folded, "", indent);
        let text = format!("{start}{folded}{end}");
        let (fbody, overflow) = fold_flow_lines(&text, indent, Fold::Block, fold_options(ctx, true));
        if !overflow {
            return format!(">{header}\n{indent}{fbody}");
        }
    }
    let val = replace_newline_runs(body, "", indent);
    format!("|{header}\n{indent}{start}{val}{end}")
}

fn plain_needs_quote_or_block(v: &str) -> bool {
    let b = v.as_bytes();
    if let Some(&c) = b.first() {
        if matches!(
            c,
            b'\n' | b'\t' | b' ' | b',' | b'[' | b']' | b'{' | b'}' | b'#' | b'&' | b'*' | b'!' | b'|' | b'>' | b'\'' | b'"' | b'%' | b'@' | b'`'
        ) {
            return true;
        }
        if matches!(c, b'?' | b'-') && (b.len() == 1 || matches!(b[1], b' ' | b'\t')) {
            return true;
        }
        if matches!(b[b.len() - 1], b'\n' | b'\t' | b' ' | b':') {
            return true;
        }
    }
    b.windows(2).any(|w| {
        matches!(w, [b'\n' | b':', b' ' | b'\t']) || matches!(w, [b' ' | b'\t', b'\n']) || matches!(w, [b'\n' | b'\t' | b' ', b'#'])
    })
}

fn plain_string(value: &str, ctx: &SCtx) -> String {
    let in_flow = ctx.in_flow == Some(true);
    if (ctx.implicit_key && value.contains('\n'))
        || (in_flow && value.contains(['[', ']', '{', '}', ',']))
    {
        return quoted_string(value, ctx);
    }
    if plain_needs_quote_or_block(value) {
        return if ctx.implicit_key || in_flow || !value.contains('\n') {
            quoted_string(value, ctx)
        } else {
            block_string(value, ctx)
        };
    }
    if !ctx.implicit_key && !in_flow && value.contains('\n') {
        return block_string(value, ctx);
    }
    if contains_document_marker(value) {
        if ctx.indent.is_empty() {
            let mut c = ctx.clone();
            c.force_block_indent = true;
            return block_string(value, &c);
        } else if ctx.implicit_key && ctx.indent == INDENT_STEP {
            return quoted_string(value, ctx);
        }
    }
    let s = replace_newline_runs(value, "", &format!("\n{}", ctx.indent));
    if test_null(&s)
        || test_bool(&s)
        || test_oct(&s)
        || test_int(&s)
        || test_hex(&s)
        || test_float_nan(&s)
        || test_float_exp(&s)
        || test_float(&s)
    {
        return quoted_string(value, ctx);
    }
    if ctx.implicit_key {
        s
    } else {
        fold_flow_lines(&s, &ctx.indent, Fold::Flow, fold_options(ctx, false)).0
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Fold {
    Flow,
    Block,
    Quoted,
}

fn consume_more_indented_lines(text: &[u16], mut i: i64, indent: i64) -> i64 {
    let at = |k: i64| if k < 0 { None } else { text.get(k as usize).copied() };
    let mut end = i;
    let mut start = i + 1;
    let mut ch = at(start);
    while matches!(ch, Some(0x20 | 0x09)) {
        if i < start + indent {
            i += 1;
            ch = at(i);
        } else {
            loop {
                i += 1;
                ch = at(i);
                if ch.is_none() || ch == Some(0x0A) {
                    break;
                }
            }
            end = i;
            start = i + 1;
            ch = at(start);
        }
    }
    end
}

/// `foldFlowLines(text, indent, mode, { indentAtStart })`, with the overflow flag.
fn fold_flow_lines(text_s: &str, indent: &str, mode: Fold, indent_at_start: Option<usize>) -> (String, bool) {
    let indent_len = utf16_len(indent) as i64;
    let line_width = LINE_WIDTH as i64;
    let min_content = MIN_CONTENT_WIDTH as i64;
    let end_step = (1 + min_content).max(1 + line_width - indent_len);
    if utf16_len(text_s) as i64 <= end_step {
        return (text_s.to_string(), false);
    }
    let text: Vec<u16> = text_s.encode_utf16().collect();
    let at = |k: i64| if k < 0 { None } else { text.get(k as usize).copied() };
    let mut folds: Vec<i64> = Vec::new();
    let mut escaped_folds: Vec<i64> = Vec::new();
    let mut end = line_width - indent_len;
    if let Some(ias) = indent_at_start {
        if ias as i64 > line_width - 2.max(min_content) {
            folds.push(0);
        } else {
            end = line_width - ias as i64;
        }
    }
    let mut split: Option<i64> = None;
    let mut prev: Option<u16> = None;
    let mut overflow = false;
    let mut i: i64 = -1;
    let (mut esc_start, mut esc_end) = (-1i64, -1i64);
    if mode == Fold::Block {
        i = consume_more_indented_lines(&text, i, indent_len);
        if i != -1 {
            end = i + end_step;
        }
    }
    let is_ws = |c: Option<u16>| matches!(c, Some(0x20 | 0x09 | 0x0A));
    loop {
        i += 1;
        let Some(mut ch) = at(i) else { break };
        if mode == Fold::Quoted && ch == 0x5C {
            esc_start = i;
            i += match at(i + 1) {
                Some(0x78) => 3,
                Some(0x75) => 5,
                Some(0x55) => 9,
                _ => 1,
            };
            esc_end = i;
        }
        if ch == 0x0A {
            if mode == Fold::Block {
                i = consume_more_indented_lines(&text, i, indent_len);
            }
            end = i + indent_len + end_step;
            split = None;
        } else {
            if ch == 0x20 && prev.is_some() && !is_ws(prev) {
                let next = at(i + 1);
                if next.is_some() && !is_ws(next) {
                    split = Some(i);
                }
            }
            if i >= end {
                if let Some(sp) = split.filter(|s| *s != 0) {
                    folds.push(sp);
                    end = sp + end_step;
                    split = None;
                } else if mode == Fold::Quoted {
                    while matches!(prev, Some(0x20 | 0x09)) {
                        prev = Some(ch);
                        i += 1;
                        ch = at(i).unwrap_or(0);
                        overflow = true;
                    }
                    let j = if i > esc_end + 1 { i - 2 } else { esc_start - 1 };
                    if escaped_folds.contains(&j) {
                        return (text_s.to_string(), overflow);
                    }
                    folds.push(j);
                    escaped_folds.push(j);
                    end = j + end_step;
                    split = None;
                } else {
                    overflow = true;
                }
            }
        }
        prev = Some(ch);
    }
    if folds.is_empty() {
        return (text_s.to_string(), overflow);
    }
    let slice = |a: i64, b: i64| -> Vec<u16> {
        let a = a.max(0) as usize;
        let b = (b.max(0) as usize).min(text.len());
        if a >= b {
            Vec::new()
        } else {
            text[a..b].to_vec()
        }
    };
    let mut res = slice(0, folds[0]);
    let indent_u: Vec<u16> = indent.encode_utf16().collect();
    for (k, &fold) in folds.iter().enumerate() {
        let e = folds.get(k + 1).copied().filter(|f| *f != 0).unwrap_or(text.len() as i64);
        if fold == 0 {
            res = vec![0x0A];
            res.extend_from_slice(&indent_u);
            res.extend_from_slice(&slice(0, e));
        } else {
            if mode == Fold::Quoted && escaped_folds.contains(&fold) {
                if let Some(c) = at(fold) {
                    res.push(c);
                }
                res.push(0x5C);
            }
            res.push(0x0A);
            res.extend_from_slice(&indent_u);
            res.extend_from_slice(&slice(fold + 1, e));
        }
    }
    (String::from_utf16_lossy(&res), overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> String {
        parse(s).unwrap().to_json()
    }

    #[test]
    fn parses_core() {
        assert_eq!(p("a: 1\nb: [x, 'y', \"z\\n\"]\nc: {d: ~}\n"), r#"{"a":1,"b":["x","y","z\n"],"c":{"d":null}}"#);
        assert_eq!(p("- 0x10\n- 0o7\n- .inf\n- 1e3\n- True\n"), "[16,7,null,1000,true]");
        assert_eq!(p("a: |\n  x\n  y\nb: >-\n  x\n  y\n"), r#"{"a":"x\ny\n","b":"x y"}"#);
        assert_eq!(p("a: &x 1\nb: *x\n"), r#"{"a":1,"b":1}"#);
    }

    #[test]
    fn errors() {
        let e = parse("a: *x\n").unwrap_err();
        assert_eq!(e.name, "ReferenceError");
        assert_eq!(e.message, "Unresolved alias (the anchor must be set before the alias): x");
        let e = parse("a: 1\na: 2\n").unwrap_err();
        assert_eq!(e.code, Some("DUPLICATE_KEY"));
        assert!(e.message.starts_with("Map keys must be unique at line 2, column 1"));
        assert_eq!(parse("a\n---\nb\n").unwrap_err().code, Some("MULTIPLE_DOCS"));
    }

    #[test]
    fn stringifies() {
        let v = parse("a: 1\nb:\n  - x\n  - y: 2\n    z: []\nc: {}\nd: null\ne: 'true'\n").unwrap();
        assert_eq!(stringify(&v), "a: 1\nb:\n  - x\n  - y: 2\n    z: []\nc: {}\nd: null\ne: \"true\"\n");
    }
}
