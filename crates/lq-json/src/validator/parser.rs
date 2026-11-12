//! The language service's own `parse` (`parser/jsonParser.js`): a fault-tolerant
//! AST with its syntax errors. Offsets are bytes. Nodes live in one arena and child
//! lists in one flat vector, so a parse does no per-node allocation.

use super::{JsError, Severity};
use crate::js::is_js_whitespace;
use crate::jsonc::{ScanError, Scanner, SyntaxKind};
use std::borrow::Cow;
use std::collections::HashMap;

pub(super) const NONE: u32 = u32::MAX;

/// Above this many distinct keys an object gets a hash map instead of linear search.
const MAP_THRESHOLD: usize = 16;
/// Nesting beyond this throws `RangeError`, about where V8 runs out of stack in the
/// recursive descent. V8's exact limit moves with what the JIT has optimized: arrays
/// measure between 3100 and 4800 levels, objects from 1800.
pub(super) const MAX_PARSE_DEPTH: u32 = 3400;
/// Levels of recursion run on one stack before hopping to a fresh one.
const STACK_HOP: u32 = 128;

#[derive(Debug)]
pub(super) enum Kind<'s> {
    Null,
    Bool,
    Number {
        value: f64,
        is_integer: bool,
    },
    String(Cow<'s, str>),
    Array {
        kids: (u32, u32),
    },
    Object {
        props: (u32, u32),
        uniq: (u32, u32),
        map: u32,
    },
    Property {
        key: u32,
        value: u32,
        dup_reported: bool,
    },
}

#[derive(Debug)]
pub(super) struct Node<'s> {
    pub offset: u32,
    pub length: u32,
    pub parent: u32,
    pub kind: Kind<'s>,
    pub bool_value: bool,
}

/// One `seenKeys` entry: first occurrence's key node, and the value of the last
/// occurrence (`NONE` for `undefined`).
#[derive(Clone, Copy, Debug)]
pub(super) struct Uniq {
    pub key: u32,
    pub value: u32,
}

#[derive(Debug)]
pub(super) struct SyntaxProblem {
    pub start: u32,
    pub end: u32,
    pub message: &'static str,
    pub severity: Severity,
    pub trailing_comma: bool,
}

#[derive(Debug)]
pub(super) struct Doc<'s> {
    pub text: &'s str,
    pub nodes: Vec<Node<'s>>,
    pub kids: Vec<u32>,
    pub uniq: Vec<Uniq>,
    pub maps: Vec<HashMap<Cow<'s, str>, u32>>,
    pub root: u32,
    pub problems: Vec<SyntaxProblem>,
}

impl<'s> Doc<'s> {
    pub fn str_value(&self, n: u32) -> &str {
        match &self.nodes[n as usize].kind {
            Kind::String(s) => s,
            _ => "",
        }
    }

    pub fn props(&self, obj: u32) -> &[u32] {
        match self.nodes[obj as usize].kind {
            Kind::Object { props: (a, b), .. } => &self.kids[a as usize..b as usize],
            _ => &[],
        }
    }

    pub fn items(&self, arr: u32) -> &[u32] {
        match self.nodes[arr as usize].kind {
            Kind::Array { kids: (a, b) } => &self.kids[a as usize..b as usize],
            _ => &[],
        }
    }

    pub fn property(&self, prop: u32) -> (u32, u32) {
        match self.nodes[prop as usize].kind {
            Kind::Property { key, value, .. } => (key, value),
            _ => (NONE, NONE),
        }
    }

    pub fn uniq(&self, obj: u32) -> &[Uniq] {
        match self.nodes[obj as usize].kind {
            Kind::Object { uniq: (a, b), .. } => &self.uniq[a as usize..b as usize],
            _ => &[],
        }
    }

    /// The index in [`Self::uniq`] of `seenKeys[name]`'s entry.
    pub fn find(&self, obj: u32, name: &str) -> Option<usize> {
        let Kind::Object {
            uniq: (a, b), map, ..
        } = self.nodes[obj as usize].kind
        else {
            return None;
        };
        if map != NONE {
            return self.maps[map as usize].get(name).map(|&i| i as usize);
        }
        self.uniq[a as usize..b as usize]
            .iter()
            .position(|u| self.str_value(u.key) == name)
    }

    /// `seenKeys[name]`: the value node, when that is not `undefined`.
    pub fn seen(&self, obj: u32, name: &str) -> Option<u32> {
        let i = self.find(obj, name)?;
        let v = self.uniq(obj)[i].value;
        (v != NONE).then_some(v)
    }
}

struct Parser<'s> {
    text: &'s str,
    scanner: Scanner<'s>,
    doc: Doc<'s>,
    /// Finished children of the containers being parsed, innermost last.
    pending: Vec<u32>,
    last_problem_offset: i64,
}

/// Runs `f` on a fresh stack. Deep recursion stays possible on small thread stacks,
/// like the rayon workers', without any cost at ordinary depths.
pub(super) fn on_fresh_stack<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    // No threads on wasm: the module is linked with one large stack instead.
    #[cfg(target_family = "wasm")]
    return f();
    #[cfg(not(target_family = "wasm"))]
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(4 << 20)
            .spawn_scoped(s, f)
            .expect("spawn stack thread")
            .join()
            .expect("stack thread")
    })
}

impl<'s> Parser<'s> {
    fn token(&self) -> SyntaxKind {
        self.scanner.token()
    }

    fn scan_next(&mut self) -> SyntaxKind {
        loop {
            let token = self.scanner.scan();
            self.check_scan_error();
            match token {
                SyntaxKind::LineComment
                | SyntaxKind::BlockComment
                | SyntaxKind::Trivia
                | SyntaxKind::LineBreak => {}
                _ => return token,
            }
        }
    }

    fn error_at_range(
        &mut self,
        message: &'static str,
        start: u32,
        end: u32,
        severity: Severity,
        trailing_comma: bool,
    ) {
        if self.doc.problems.is_empty() || i64::from(start) != self.last_problem_offset {
            self.doc.problems.push(SyntaxProblem {
                start,
                end,
                message,
                severity,
                trailing_comma,
            });
            self.last_problem_offset = i64::from(start);
        }
    }

    /// `_error` without its optional arguments.
    fn error(&mut self, message: &'static str) {
        let mut start = self.scanner.token_offset();
        let mut end = start + self.scanner.token_length();
        if start == end && start > 0 {
            let char_before = |i: usize| self.text[..i].chars().next_back().unwrap_or('\0');
            let char_at = |i: usize| self.text[i..].chars().next().unwrap_or('\0');
            start -= char_before(start).len_utf8();
            while start > 0 && is_js_whitespace(char_at(start)) {
                start -= char_before(start).len_utf8();
            }
            let c = char_at(start);
            end = start + c.len_utf8();
            if c.len_utf16() == 2 {
                // `start--` works in code units, and this one stopped on a low surrogate.
                // That is a position of its own to `_errorAtRange` and `addProblem`, so
                // keep it distinct: two bytes in. Reporting floors it to the character.
                start += 2;
            }
        }
        self.error_at_range(message, start as u32, end as u32, Severity::Error, false);
    }

    fn skip_until(&mut self, stop: [SyntaxKind; 2]) {
        let mut token = self.token();
        while token != SyntaxKind::Eof {
            if stop.contains(&token) {
                break;
            }
            token = self.scan_next();
        }
    }

    fn check_scan_error(&mut self) {
        let message = match self.scanner.token_error() {
            ScanError::None => return,
            ScanError::InvalidUnicode => "Invalid unicode sequence in string.",
            ScanError::InvalidEscapeCharacter => "Invalid escape character in string.",
            ScanError::UnexpectedEndOfNumber => "Unexpected end of number.",
            ScanError::UnexpectedEndOfComment => "Unexpected end of comment.",
            ScanError::UnexpectedEndOfString => "Unexpected end of string.",
            ScanError::InvalidCharacter => {
                "Invalid characters in string. Control characters must be escaped."
            }
        };
        self.error(message);
    }

    fn finalize(&mut self, id: u32, scan_next: bool) {
        let end = self.scanner.token_offset() + self.scanner.token_length();
        let node = &mut self.doc.nodes[id as usize];
        node.length = end as u32 - node.offset;
        if scan_next {
            self.scan_next();
        }
    }

    fn new_node(&mut self, parent: u32, offset: usize, kind: Kind<'s>) -> u32 {
        let id = self.doc.nodes.len() as u32;
        self.doc.nodes.push(Node {
            offset: offset as u32,
            length: 0,
            parent,
            kind,
            bool_value: false,
        });
        id
    }

    fn flush_kids(&mut self, base: usize) -> (u32, u32) {
        let a = self.doc.kids.len() as u32;
        self.doc.kids.extend_from_slice(&self.pending[base..]);
        self.pending.truncate(base);
        (a, self.doc.kids.len() as u32)
    }

    fn parse_array(&mut self, parent: u32, depth: u32) -> Result<Option<u32>, JsError> {
        if self.token() != SyntaxKind::OpenBracket {
            return Ok(None);
        }
        let id = self.new_node(
            parent,
            self.scanner.token_offset(),
            Kind::Array { kids: (0, 0) },
        );
        self.scan_next();
        let base = self.pending.len();
        let mut needs_comma = false;
        while self.token() != SyntaxKind::CloseBracket && self.token() != SyntaxKind::Eof {
            if self.token() == SyntaxKind::Comma {
                if !needs_comma {
                    self.error("Value expected");
                }
                let comma = self.scanner.token_offset() as u32;
                self.scan_next();
                if self.token() == SyntaxKind::CloseBracket {
                    if needs_comma {
                        self.error_at_range(
                            "Trailing comma",
                            comma,
                            comma + 1,
                            Severity::Error,
                            true,
                        );
                    }
                    continue;
                }
            } else if needs_comma {
                self.error("Expected comma");
            }
            match self.parse_value(id, depth + 1)? {
                None => {
                    self.error("Value expected");
                    self.skip_until([SyntaxKind::CloseBracket, SyntaxKind::Comma]);
                }
                Some(item) => self.pending.push(item),
            }
            needs_comma = true;
        }
        let kids = self.flush_kids(base);
        self.doc.nodes[id as usize].kind = Kind::Array { kids };
        if self.token() != SyntaxKind::CloseBracket {
            self.error("Expected comma or closing bracket");
            self.finalize(id, false);
            return Ok(Some(id));
        }
        self.finalize(id, true);
        Ok(Some(id))
    }

    fn parse_property(
        &mut self,
        parent: u32,
        base: usize,
        keys_seen: &mut Option<HashMap<Cow<'s, str>, u32>>,
        depth: u32,
    ) -> Result<Option<u32>, JsError> {
        let id = self.new_node(
            parent,
            self.scanner.token_offset(),
            Kind::Property {
                key: NONE,
                value: NONE,
                dup_reported: false,
            },
        );
        let key = match self.parse_string(id) {
            Some(key) => key,
            None => {
                if self.token() != SyntaxKind::Unknown {
                    return Ok(None);
                }
                self.error("Property keys must be doublequoted");
                let offset = self.scanner.token_offset();
                let length = self.scanner.token_length();
                let value = self.scanner.take_token_value();
                let key = self.new_node(id, offset, Kind::String(value));
                self.doc.nodes[key as usize].length = length as u32;
                self.scan_next();
                key
            }
        };
        self.doc.nodes[id as usize].kind = Kind::Property {
            key,
            value: NONE,
            dup_reported: false,
        };
        self.note_key(key, id, base, keys_seen);

        if self.token() == SyntaxKind::Colon {
            self.scan_next();
        } else {
            self.error("Colon expected");
            if self.token() == SyntaxKind::StringLiteral {
                let k = &self.doc.nodes[key as usize];
                let from = (k.offset + k.length) as usize;
                if crosses_line(self.text.as_bytes(), from, self.scanner.token_offset()) {
                    self.doc.nodes[id as usize].length = self.doc.nodes[key as usize].length;
                    return Ok(Some(id));
                }
            }
        }
        let Some(value) = self.parse_value(id, depth + 1)? else {
            self.error("Value expected");
            self.finalize(id, false);
            self.skip_until([SyntaxKind::CloseBrace, SyntaxKind::Comma]);
            return Ok(Some(id));
        };
        if let Kind::Property { value: slot, .. } = &mut self.doc.nodes[id as usize].kind {
            *slot = value;
        }
        let v = &self.doc.nodes[value as usize];
        let end = v.offset + v.length;
        let node = &mut self.doc.nodes[id as usize];
        node.length = end - node.offset;
        Ok(Some(id))
    }

    /// The `keysSeen` part of `_parseProperty`.
    fn note_key(
        &mut self,
        key: u32,
        prop: u32,
        base: usize,
        keys_seen: &mut Option<HashMap<Cow<'s, str>, u32>>,
    ) {
        let (name, offset, length) = {
            let k = &self.doc.nodes[key as usize];
            let Kind::String(s) = &k.kind else { return };
            (s.clone(), k.offset, k.length)
        };
        if name == "//" {
            return;
        }
        let first = if let Some(map) = keys_seen {
            map.get(name.as_ref()).copied()
        } else {
            self.pending[base..].iter().copied().find(|&p| {
                let (k, _) = self.doc.property(p);
                self.doc.str_value(k) == name
            })
        };
        let Some(first) = first else {
            let count = self.pending.len() - base;
            if keys_seen.is_none() && count >= 32 {
                let mut map = HashMap::with_capacity(count * 2);
                for &p in &self.pending[base..] {
                    let (k, _) = self.doc.property(p);
                    if let Kind::String(s) = &self.doc.nodes[k as usize].kind {
                        map.entry(s.clone()).or_insert(p);
                    }
                }
                *keys_seen = Some(map);
            }
            if let Some(map) = keys_seen {
                map.insert(name, prop);
            }
            return;
        };
        self.error_at_range(
            "Duplicate object key",
            offset,
            offset + length,
            Severity::Warning,
            false,
        );
        let Kind::Property {
            key: first_key,
            dup_reported,
            ..
        } = self.doc.nodes[first as usize].kind
        else {
            return;
        };
        if !dup_reported {
            let k = &self.doc.nodes[first_key as usize];
            let (o, l) = (k.offset, k.length);
            self.error_at_range("Duplicate object key", o, o + l, Severity::Warning, false);
            if let Kind::Property { dup_reported, .. } = &mut self.doc.nodes[first as usize].kind {
                *dup_reported = true;
            }
        }
    }

    fn parse_object(&mut self, parent: u32, depth: u32) -> Result<Option<u32>, JsError> {
        if self.token() != SyntaxKind::OpenBrace {
            return Ok(None);
        }
        let id = self.new_node(
            parent,
            self.scanner.token_offset(),
            Kind::Object {
                props: (0, 0),
                uniq: (0, 0),
                map: NONE,
            },
        );
        self.scan_next();
        let base = self.pending.len();
        let mut keys_seen = None;
        let mut needs_comma = false;
        while self.token() != SyntaxKind::CloseBrace && self.token() != SyntaxKind::Eof {
            if self.token() == SyntaxKind::Comma {
                if !needs_comma {
                    self.error("Property expected");
                }
                let comma = self.scanner.token_offset() as u32;
                self.scan_next();
                if self.token() == SyntaxKind::CloseBrace {
                    if needs_comma {
                        self.error_at_range(
                            "Trailing comma",
                            comma,
                            comma + 1,
                            Severity::Error,
                            true,
                        );
                    }
                    continue;
                }
            } else if needs_comma {
                self.error("Expected comma");
            }
            match self.parse_property(id, base, &mut keys_seen, depth)? {
                None => {
                    self.error("Property expected");
                    self.skip_until([SyntaxKind::CloseBrace, SyntaxKind::Comma]);
                }
                Some(prop) => self.pending.push(prop),
            }
            needs_comma = true;
        }
        let props = self.flush_kids(base);
        let (uniq, map) = self.build_uniq(props);
        self.doc.nodes[id as usize].kind = Kind::Object { props, uniq, map };
        if self.token() != SyntaxKind::CloseBrace {
            self.error("Expected comma or closing brace");
            self.finalize(id, false);
            return Ok(Some(id));
        }
        self.finalize(id, true);
        Ok(Some(id))
    }

    /// `seenKeys` and the order of `unprocessedProperties`, computed once per object.
    fn build_uniq(&mut self, props: (u32, u32)) -> ((u32, u32), u32) {
        let start = self.doc.uniq.len();
        let count = (props.1 - props.0) as usize;
        let doc = &mut self.doc;
        if count <= MAP_THRESHOLD {
            for i in props.0..props.1 {
                let (key, value) = doc.property(doc.kids[i as usize]);
                let name = doc.str_value(key);
                let at = doc.uniq[start..]
                    .iter()
                    .position(|u| doc.str_value(u.key) == name);
                match at {
                    Some(at) => doc.uniq[start + at].value = value,
                    None => doc.uniq.push(Uniq { key, value }),
                }
            }
            return ((start as u32, doc.uniq.len() as u32), NONE);
        }
        let mut map: HashMap<Cow<'s, str>, u32> = HashMap::with_capacity(count);
        for i in props.0..props.1 {
            let (key, value) = doc.property(doc.kids[i as usize]);
            let Kind::String(name) = &doc.nodes[key as usize].kind else {
                continue;
            };
            match map.get(name.as_ref()) {
                Some(&at) => doc.uniq[start + at as usize].value = value,
                None => {
                    map.insert(name.clone(), (doc.uniq.len() - start) as u32);
                    doc.uniq.push(Uniq { key, value });
                }
            }
        }
        doc.maps.push(map);
        (
            (start as u32, doc.uniq.len() as u32),
            doc.maps.len() as u32 - 1,
        )
    }

    fn parse_string(&mut self, parent: u32) -> Option<u32> {
        if self.token() != SyntaxKind::StringLiteral {
            return None;
        }
        let offset = self.scanner.token_offset();
        let value = self.scanner.take_token_value();
        let id = self.new_node(parent, offset, Kind::String(value));
        self.finalize(id, true);
        Some(id)
    }

    fn parse_number(&mut self, parent: u32) -> Option<u32> {
        if self.token() != SyntaxKind::NumericLiteral {
            return None;
        }
        let (mut value, mut is_integer) = (f64::NAN, true);
        if self.scanner.token_error() == ScanError::None {
            let token = self.scanner.token_value();
            value = token.parse().unwrap_or(f64::NAN);
            is_integer = !token.contains('.');
        }
        let id = self.new_node(
            parent,
            self.scanner.token_offset(),
            Kind::Number { value, is_integer },
        );
        self.finalize(id, true);
        Some(id)
    }

    fn parse_literal(&mut self, parent: u32) -> Option<u32> {
        let (kind, value) = match self.token() {
            SyntaxKind::Null => (Kind::Null, false),
            SyntaxKind::True => (Kind::Bool, true),
            SyntaxKind::False => (Kind::Bool, false),
            _ => return None,
        };
        let id = self.new_node(parent, self.scanner.token_offset(), kind);
        self.doc.nodes[id as usize].bool_value = value;
        self.finalize(id, true);
        Some(id)
    }

    fn parse_value(&mut self, parent: u32, depth: u32) -> Result<Option<u32>, JsError> {
        match self.token() {
            SyntaxKind::OpenBracket | SyntaxKind::OpenBrace => {
                if depth >= MAX_PARSE_DEPTH {
                    return Err(JsError::range_error());
                }
                if depth % STACK_HOP == STACK_HOP - 1 {
                    return on_fresh_stack(|| self.parse_container(parent, depth));
                }
                self.parse_container(parent, depth)
            }
            SyntaxKind::StringLiteral => Ok(self.parse_string(parent)),
            SyntaxKind::NumericLiteral => Ok(self.parse_number(parent)),
            _ => Ok(self.parse_literal(parent)),
        }
    }

    fn parse_container(&mut self, parent: u32, depth: u32) -> Result<Option<u32>, JsError> {
        if self.token() == SyntaxKind::OpenBracket {
            self.parse_array(parent, depth)
        } else {
            self.parse_object(parent, depth)
        }
    }
}

/// Whether the offsets lie on different lines, per `computeLineOffsets`.
fn crosses_line(bytes: &[u8], from: usize, to: usize) -> bool {
    bytes[from..to]
        .iter()
        .enumerate()
        .any(|(i, &b)| b == b'\n' || (b == b'\r' && bytes.get(from + i + 1) != Some(&b'\n')))
}

pub(super) fn parse(text: &str) -> Result<Doc<'_>, JsError> {
    let mut p = Parser {
        text,
        scanner: Scanner::new(text),
        doc: Doc {
            text,
            nodes: Vec::with_capacity(text.len() / 12 + 4),
            kids: Vec::with_capacity(text.len() / 16 + 4),
            uniq: Vec::new(),
            maps: Vec::new(),
            root: NONE,
            problems: Vec::new(),
        },
        pending: Vec::new(),
        last_problem_offset: -1,
    };
    let token = p.scan_next();
    if token != SyntaxKind::Eof {
        match p.parse_value(NONE, 0)? {
            None => p.error("Expected a JSON object, array or literal."),
            Some(root) => {
                p.doc.root = root;
                if p.token() != SyntaxKind::Eof {
                    p.error("End of file expected.");
                }
            }
        }
    }
    Ok(p.doc)
}
