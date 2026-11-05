//! Port of `jsonc-parser` 3.3.1: the scanner, `visit`, `parse` and `parseTree`.
//! Offsets are byte offsets; upstream's are UTF-16.

use crate::value::{Object, Value};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntaxKind {
    OpenBrace = 1,
    CloseBrace = 2,
    OpenBracket = 3,
    CloseBracket = 4,
    Comma = 5,
    Colon = 6,
    Null = 7,
    True = 8,
    False = 9,
    StringLiteral = 10,
    NumericLiteral = 11,
    LineComment = 12,
    BlockComment = 13,
    LineBreak = 14,
    Trivia = 15,
    Unknown = 16,
    Eof = 17,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanError {
    None = 0,
    UnexpectedEndOfComment = 1,
    UnexpectedEndOfString = 2,
    UnexpectedEndOfNumber = 3,
    InvalidUnicode = 4,
    InvalidEscapeCharacter = 5,
    InvalidCharacter = 6,
}

/// `createScanner(text, false)`: trivia tokens are returned, as both `visit` and
/// the language service's parser want them.
pub struct Scanner<'s> {
    text: &'s str,
    bytes: &'s [u8],
    pos: usize,
    token_offset: usize,
    token: SyntaxKind,
    error: ScanError,
    value: Cow<'s, str>,
}

#[inline]
fn is_ws(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

#[inline]
fn is_lb(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

impl<'s> Scanner<'s> {
    pub fn new(text: &'s str) -> Self {
        Scanner {
            text,
            bytes: text.as_bytes(),
            pos: 0,
            token_offset: 0,
            token: SyntaxKind::Unknown,
            error: ScanError::None,
            value: Cow::Borrowed(""),
        }
    }

    pub fn token(&self) -> SyntaxKind {
        self.token
    }

    pub fn token_value(&self) -> &str {
        &self.value
    }

    pub fn take_token_value(&mut self) -> Cow<'s, str> {
        std::mem::replace(&mut self.value, Cow::Borrowed(""))
    }

    pub fn token_offset(&self) -> usize {
        self.token_offset
    }

    pub fn token_length(&self) -> usize {
        self.pos - self.token_offset
    }

    pub fn token_error(&self) -> ScanError {
        self.error
    }

    fn at(&self, i: usize) -> Option<u8> {
        self.bytes.get(i).copied()
    }

    fn scan_hex4(&mut self) -> Option<u32> {
        let mut value = 0u32;
        for digits in 0..4 {
            let d = match self.at(self.pos) {
                Some(c @ b'0'..=b'9') => u32::from(c - b'0'),
                Some(c @ b'A'..=b'F') => u32::from(c - b'A') + 10,
                Some(c @ b'a'..=b'f') => u32::from(c - b'a') + 10,
                _ => {
                    let _ = digits;
                    return None;
                }
            };
            value = value * 16 + d;
            self.pos += 1;
        }
        Some(value)
    }

    fn scan_number(&mut self) -> &'s str {
        let start = self.pos;
        let digit = |s: &Self, i: usize| s.at(i).is_some_and(|c| c.is_ascii_digit());
        if self.at(self.pos) == Some(b'0') {
            self.pos += 1;
        } else {
            self.pos += 1;
            while digit(self, self.pos) {
                self.pos += 1;
            }
        }
        if self.at(self.pos) == Some(b'.') {
            self.pos += 1;
            if digit(self, self.pos) {
                self.pos += 1;
                while digit(self, self.pos) {
                    self.pos += 1;
                }
            } else {
                self.error = ScanError::UnexpectedEndOfNumber;
                return &self.text[start..self.pos];
            }
        }
        let mut end = self.pos;
        if matches!(self.at(self.pos), Some(b'E' | b'e')) {
            self.pos += 1;
            if matches!(self.at(self.pos), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if digit(self, self.pos) {
                self.pos += 1;
                while digit(self, self.pos) {
                    self.pos += 1;
                }
                end = self.pos;
            } else {
                self.error = ScanError::UnexpectedEndOfNumber;
            }
        }
        &self.text[start..end]
    }

    fn scan_string(&mut self) -> Cow<'s, str> {
        let mut result: Option<String> = None;
        let mut start = self.pos;
        let mut pending_high: Option<u32> = None;
        let len = self.bytes.len();
        macro_rules! flush_high {
            ($r:expr) => {
                if let Some(h) = pending_high.take() {
                    let _ = h;
                    // A lone surrogate has no UTF-8 form.
                    $r.push('\u{FFFD}');
                }
            };
        }
        loop {
            if self.pos >= len {
                let tail = &self.text[start..self.pos];
                self.error = ScanError::UnexpectedEndOfString;
                return match result {
                    Some(mut r) => {
                        flush_high!(r);
                        r.push_str(tail);
                        Cow::Owned(r)
                    }
                    None => Cow::Borrowed(tail),
                };
            }
            let ch = self.bytes[self.pos];
            if ch == b'"' {
                let tail = &self.text[start..self.pos];
                self.pos += 1;
                return match result {
                    Some(mut r) => {
                        if !tail.is_empty() {
                            flush_high!(r);
                        }
                        flush_high!(r);
                        r.push_str(tail);
                        Cow::Owned(r)
                    }
                    None => Cow::Borrowed(tail),
                };
            }
            if ch == b'\\' {
                let r = result.get_or_insert_with(String::new);
                let tail = &self.text[start..self.pos];
                if !tail.is_empty() {
                    flush_high!(r);
                    r.push_str(tail);
                }
                self.pos += 1;
                if self.pos >= len {
                    self.error = ScanError::UnexpectedEndOfString;
                    flush_high!(r);
                    return Cow::Owned(result.unwrap());
                }
                let ch2 = self.bytes[self.pos];
                self.pos += 1;
                let simple = match ch2 {
                    b'"' => Some('"'),
                    b'\\' => Some('\\'),
                    b'/' => Some('/'),
                    b'b' => Some('\u{8}'),
                    b'f' => Some('\u{c}'),
                    b'n' => Some('\n'),
                    b'r' => Some('\r'),
                    b't' => Some('\t'),
                    _ => None,
                };
                if let Some(c) = simple {
                    flush_high!(r);
                    r.push(c);
                } else if ch2 == b'u' {
                    // The hex scan needs `self` while `r` borrows `result`.
                    let code = {
                        let saved = std::mem::take(r);
                        let code = self.scan_hex4();
                        *result.as_mut().unwrap() = saved;
                        code
                    };
                    let r = result.as_mut().unwrap();
                    match code {
                        Some(code @ 0xD800..=0xDBFF) => {
                            flush_high!(r);
                            pending_high = Some(code);
                        }
                        Some(code @ 0xDC00..=0xDFFF) => {
                            if let Some(h) = pending_high.take() {
                                let c = 0x10000 + ((h - 0xD800) << 10) + (code - 0xDC00);
                                r.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                            } else {
                                r.push('\u{FFFD}');
                            }
                        }
                        Some(code) => {
                            flush_high!(r);
                            r.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                        }
                        None => self.error = ScanError::InvalidUnicode,
                    }
                } else {
                    // The bad character is consumed and dropped. It may be multi-byte.
                    self.error = ScanError::InvalidEscapeCharacter;
                    if ch2 >= 0x80 {
                        self.pos -= 1;
                        self.pos += self.text[self.pos..].chars().next().map_or(1, char::len_utf8);
                    }
                }
                start = self.pos;
                continue;
            }
            if ch <= 0x1f {
                if is_lb(ch) {
                    let tail = &self.text[start..self.pos];
                    self.error = ScanError::UnexpectedEndOfString;
                    return match result {
                        Some(mut r) => {
                            flush_high!(r);
                            r.push_str(tail);
                            Cow::Owned(r)
                        }
                        None => Cow::Borrowed(tail),
                    };
                }
                self.error = ScanError::InvalidCharacter;
            }
            self.pos += 1;
        }
    }

    pub fn scan(&mut self) -> SyntaxKind {
        self.token = self.scan_next();
        self.token
    }

    fn scan_next(&mut self) -> SyntaxKind {
        self.value = Cow::Borrowed("");
        self.error = ScanError::None;
        self.token_offset = self.pos;
        let len = self.bytes.len();
        if self.pos >= len {
            self.token_offset = len;
            return SyntaxKind::Eof;
        }
        let code = self.bytes[self.pos];
        if is_ws(code) {
            while self.pos < len && is_ws(self.bytes[self.pos]) {
                self.pos += 1;
            }
            self.value = Cow::Borrowed(&self.text[self.token_offset..self.pos]);
            return SyntaxKind::Trivia;
        }
        if is_lb(code) {
            self.pos += 1;
            if code == b'\r' && self.at(self.pos) == Some(b'\n') {
                self.pos += 1;
            }
            self.value = Cow::Borrowed(&self.text[self.token_offset..self.pos]);
            return SyntaxKind::LineBreak;
        }
        match code {
            b'{' => {
                self.pos += 1;
                SyntaxKind::OpenBrace
            }
            b'}' => {
                self.pos += 1;
                SyntaxKind::CloseBrace
            }
            b'[' => {
                self.pos += 1;
                SyntaxKind::OpenBracket
            }
            b']' => {
                self.pos += 1;
                SyntaxKind::CloseBracket
            }
            b':' => {
                self.pos += 1;
                SyntaxKind::Colon
            }
            b',' => {
                self.pos += 1;
                SyntaxKind::Comma
            }
            b'"' => {
                self.pos += 1;
                self.value = self.scan_string();
                SyntaxKind::StringLiteral
            }
            b'/' => {
                let next = self.at(self.pos + 1);
                if next == Some(b'/') {
                    self.pos += 2;
                    while self.pos < len && !is_lb(self.bytes[self.pos]) {
                        self.pos += 1;
                    }
                    return SyntaxKind::LineComment;
                }
                if next == Some(b'*') {
                    self.pos += 2;
                    let safe = len - 1;
                    let mut closed = false;
                    while self.pos < safe {
                        let ch = self.bytes[self.pos];
                        if ch == b'*' && self.bytes[self.pos + 1] == b'/' {
                            self.pos += 2;
                            closed = true;
                            break;
                        }
                        self.pos += 1;
                        if ch == b'\r' && self.at(self.pos) == Some(b'\n') {
                            self.pos += 1;
                        }
                    }
                    if !closed {
                        // Upstream steps one code unit past, even past the end: a `/*` that
                        // ends the text leaves an EOF token of length 1.
                        self.pos += 1;
                        while self.pos < len && !self.text.is_char_boundary(self.pos) {
                            self.pos += 1;
                        }
                        self.error = ScanError::UnexpectedEndOfComment;
                    }
                    return SyntaxKind::BlockComment;
                }
                self.pos += 1;
                self.value = Cow::Borrowed("/");
                SyntaxKind::Unknown
            }
            b'-' | b'0'..=b'9' => {
                if code == b'-' {
                    self.pos += 1;
                    if self.pos == len || !self.bytes[self.pos].is_ascii_digit() {
                        self.value = Cow::Borrowed("-");
                        return SyntaxKind::Unknown;
                    }
                    let digits = self.scan_number();
                    // `-` plus the digits, adjacent in the source unless an
                    // exponent was dropped; both are a prefix of the token.
                    let end = self.token_offset + 1 + digits.len();
                    self.value = Cow::Borrowed(&self.text[self.token_offset..end]);
                } else {
                    self.value = Cow::Borrowed(self.scan_number());
                }
                SyntaxKind::NumericLiteral
            }
            _ => {
                while self.pos < len && is_unknown_content(self.bytes[self.pos]) {
                    self.pos += 1;
                }
                if self.token_offset != self.pos {
                    let word = &self.text[self.token_offset..self.pos];
                    self.value = Cow::Borrowed(word);
                    return match word {
                        "true" => SyntaxKind::True,
                        "false" => SyntaxKind::False,
                        "null" => SyntaxKind::Null,
                        _ => SyntaxKind::Unknown,
                    };
                }
                // Unreachable for valid first bytes, kept from upstream's shape.
                let w = self.text[self.pos..].chars().next().map_or(1, char::len_utf8);
                self.value = Cow::Borrowed(&self.text[self.pos..self.pos + w]);
                self.pos += w;
                SyntaxKind::Unknown
            }
        }
    }
}

#[inline]
fn is_unknown_content(b: u8) -> bool {
    !(is_ws(b) || is_lb(b) || matches!(b, b'}' | b']' | b'{' | b'[' | b'"' | b':' | b',' | b'/'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseErrorCode {
    InvalidSymbol = 1,
    InvalidNumberFormat = 2,
    PropertyNameExpected = 3,
    ValueExpected = 4,
    ColonExpected = 5,
    CommaExpected = 6,
    CloseBraceExpected = 7,
    CloseBracketExpected = 8,
    EndOfFileExpected = 9,
    InvalidCommentToken = 10,
    UnexpectedEndOfComment = 11,
    UnexpectedEndOfString = 12,
    UnexpectedEndOfNumber = 13,
    InvalidUnicode = 14,
    InvalidEscapeCharacter = 15,
    InvalidCharacter = 16,
}

impl ParseErrorCode {
    /// `PARSE_ERROR_MESSAGES` from `utils/error.ts`.
    pub fn message(self) -> &'static str {
        match self {
            ParseErrorCode::InvalidSymbol => "Invalid symbol found in the input.",
            ParseErrorCode::InvalidNumberFormat => "Invalid number format detected.",
            ParseErrorCode::PropertyNameExpected => "Property name expected but not found.",
            ParseErrorCode::ValueExpected => "A value was expected but not found.",
            ParseErrorCode::ColonExpected => "Colon `:` expected after the property name.",
            ParseErrorCode::CommaExpected => "Comma `,` expected between elements and properties.",
            ParseErrorCode::CloseBraceExpected => "Closing brace `}` expected.",
            ParseErrorCode::CloseBracketExpected => "Closing bracket `]` expected.",
            ParseErrorCode::EndOfFileExpected => "End of file expected.",
            ParseErrorCode::InvalidCommentToken => "Invalid comment token found.",
            ParseErrorCode::UnexpectedEndOfComment => "Unexpected end of comment.",
            ParseErrorCode::UnexpectedEndOfString => "Unexpected end of string.",
            ParseErrorCode::UnexpectedEndOfNumber => "Unexpected end of number.",
            ParseErrorCode::InvalidUnicode => "Invalid Unicode escape sequence.",
            ParseErrorCode::InvalidEscapeCharacter => "Invalid escape character found.",
            ParseErrorCode::InvalidCharacter => "Invalid character found in the input.",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub error: ParseErrorCode,
    /// Byte offset.
    pub offset: u32,
    /// Byte length.
    pub length: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ParseOptions {
    pub disallow_comments: bool,
    pub allow_trailing_comma: bool,
    pub allow_empty_content: bool,
}

/// `JSONVisitor`. Offsets and lengths are in bytes.
pub trait Visitor<'s> {
    fn on_object_begin(&mut self, _offset: usize) {}
    fn on_object_property(&mut self, _name: Cow<'s, str>, _offset: usize, _length: usize) {}
    fn on_object_end(&mut self, _offset: usize, _length: usize) {}
    fn on_array_begin(&mut self, _offset: usize) {}
    fn on_array_end(&mut self, _offset: usize, _length: usize) {}
    fn on_string_value(&mut self, _value: Cow<'s, str>, _offset: usize, _length: usize) {}
    fn on_number_value(&mut self, _value: f64, _offset: usize, _length: usize) {}
    fn on_bool_value(&mut self, _value: bool, _offset: usize, _length: usize) {}
    fn on_null_value(&mut self, _offset: usize, _length: usize) {}
    fn on_separator(&mut self, _sep: char, _offset: usize) {}
    fn on_error(&mut self, _error: ParseErrorCode, _offset: usize, _length: usize) {}
}

struct Walker<'s, 'v, V: Visitor<'s>> {
    s: Scanner<'s>,
    v: &'v mut V,
    opts: ParseOptions,
}

impl<'s, V: Visitor<'s>> Walker<'s, '_, V> {
    fn error(&mut self, code: ParseErrorCode) {
        let (o, l) = (self.s.token_offset(), self.s.token_length());
        self.v.on_error(code, o, l);
    }

    fn scan_next(&mut self) -> SyntaxKind {
        loop {
            let token = self.s.scan();
            match self.s.token_error() {
                ScanError::InvalidUnicode => self.error(ParseErrorCode::InvalidUnicode),
                ScanError::InvalidEscapeCharacter => {
                    self.error(ParseErrorCode::InvalidEscapeCharacter)
                }
                ScanError::UnexpectedEndOfNumber => {
                    self.error(ParseErrorCode::UnexpectedEndOfNumber)
                }
                ScanError::UnexpectedEndOfComment => {
                    if !self.opts.disallow_comments {
                        self.error(ParseErrorCode::UnexpectedEndOfComment);
                    }
                }
                ScanError::UnexpectedEndOfString => {
                    self.error(ParseErrorCode::UnexpectedEndOfString)
                }
                ScanError::InvalidCharacter => self.error(ParseErrorCode::InvalidCharacter),
                ScanError::None => {}
            }
            match token {
                SyntaxKind::LineComment | SyntaxKind::BlockComment => {
                    if self.opts.disallow_comments {
                        self.error(ParseErrorCode::InvalidCommentToken);
                    }
                }
                SyntaxKind::Unknown => self.error(ParseErrorCode::InvalidSymbol),
                SyntaxKind::Trivia | SyntaxKind::LineBreak => {}
                _ => return token,
            }
        }
    }

    fn handle_error(
        &mut self,
        code: ParseErrorCode,
        skip_until_after: &[SyntaxKind],
        skip_until: &[SyntaxKind],
    ) {
        self.error(code);
        if !skip_until_after.is_empty() || !skip_until.is_empty() {
            let mut token = self.s.token();
            while token != SyntaxKind::Eof {
                if skip_until_after.contains(&token) {
                    self.scan_next();
                    break;
                } else if skip_until.contains(&token) {
                    break;
                }
                token = self.scan_next();
            }
        }
    }

    fn parse_literal(&mut self) -> bool {
        let (o, l) = (self.s.token_offset(), self.s.token_length());
        match self.s.token() {
            SyntaxKind::NumericLiteral => {
                let value = self.s.token_value().parse::<f64>().unwrap_or(0.0);
                self.v.on_number_value(value, o, l);
            }
            SyntaxKind::Null => self.v.on_null_value(o, l),
            SyntaxKind::True => self.v.on_bool_value(true, o, l),
            SyntaxKind::False => self.v.on_bool_value(false, o, l),
            _ => return false,
        }
        self.scan_next();
        true
    }

    fn parse_property(&mut self) -> bool {
        use SyntaxKind::{CloseBrace, Comma};
        if self.s.token() != SyntaxKind::StringLiteral {
            self.handle_error(ParseErrorCode::PropertyNameExpected, &[], &[CloseBrace, Comma]);
            return false;
        }
        let (o, l) = (self.s.token_offset(), self.s.token_length());
        let name = self.s.take_token_value();
        self.v.on_object_property(name, o, l);
        self.scan_next();
        if self.s.token() == SyntaxKind::Colon {
            let o = self.s.token_offset();
            self.v.on_separator(':', o);
            self.scan_next();
            if !self.parse_value() {
                self.handle_error(ParseErrorCode::ValueExpected, &[], &[CloseBrace, Comma]);
            }
        } else {
            self.handle_error(ParseErrorCode::ColonExpected, &[], &[CloseBrace, Comma]);
        }
        true
    }

    fn parse_object(&mut self) -> bool {
        use SyntaxKind::{CloseBrace, Comma, Eof};
        let o = self.s.token_offset();
        self.v.on_object_begin(o);
        self.scan_next();
        let mut needs_comma = false;
        while self.s.token() != CloseBrace && self.s.token() != Eof {
            if self.s.token() == Comma {
                if !needs_comma {
                    self.handle_error(ParseErrorCode::ValueExpected, &[], &[]);
                }
                let o = self.s.token_offset();
                self.v.on_separator(',', o);
                self.scan_next();
                if self.s.token() == CloseBrace && self.opts.allow_trailing_comma {
                    break;
                }
            } else if needs_comma {
                self.handle_error(ParseErrorCode::CommaExpected, &[], &[]);
            }
            if !self.parse_property() {
                self.handle_error(ParseErrorCode::ValueExpected, &[], &[CloseBrace, Comma]);
            }
            needs_comma = true;
        }
        let (o, l) = (self.s.token_offset(), self.s.token_length());
        self.v.on_object_end(o, l);
        if self.s.token() != CloseBrace {
            self.handle_error(ParseErrorCode::CloseBraceExpected, &[CloseBrace], &[]);
        } else {
            self.scan_next();
        }
        true
    }

    fn parse_array(&mut self) -> bool {
        use SyntaxKind::{CloseBracket, Comma, Eof};
        let o = self.s.token_offset();
        self.v.on_array_begin(o);
        self.scan_next();
        let mut needs_comma = false;
        while self.s.token() != CloseBracket && self.s.token() != Eof {
            if self.s.token() == Comma {
                if !needs_comma {
                    self.handle_error(ParseErrorCode::ValueExpected, &[], &[]);
                }
                let o = self.s.token_offset();
                self.v.on_separator(',', o);
                self.scan_next();
                if self.s.token() == CloseBracket && self.opts.allow_trailing_comma {
                    break;
                }
            } else if needs_comma {
                self.handle_error(ParseErrorCode::CommaExpected, &[], &[]);
            }
            if !self.parse_value() {
                self.handle_error(ParseErrorCode::ValueExpected, &[], &[CloseBracket, Comma]);
            }
            needs_comma = true;
        }
        let (o, l) = (self.s.token_offset(), self.s.token_length());
        self.v.on_array_end(o, l);
        if self.s.token() != CloseBracket {
            self.handle_error(ParseErrorCode::CloseBracketExpected, &[CloseBracket], &[]);
        } else {
            self.scan_next();
        }
        true
    }

    fn parse_value(&mut self) -> bool {
        match self.s.token() {
            SyntaxKind::OpenBracket => self.parse_array(),
            SyntaxKind::OpenBrace => self.parse_object(),
            SyntaxKind::StringLiteral => {
                let (o, l) = (self.s.token_offset(), self.s.token_length());
                let value = self.s.take_token_value();
                self.v.on_string_value(value, o, l);
                self.scan_next();
                true
            }
            _ => self.parse_literal(),
        }
    }
}

/// `visit(text, visitor, options)`.
pub fn visit<'s, V: Visitor<'s>>(text: &'s str, visitor: &mut V, opts: ParseOptions) -> bool {
    let mut w = Walker { s: Scanner::new(text), v: visitor, opts };
    w.scan_next();
    if w.s.token() == SyntaxKind::Eof {
        if opts.allow_empty_content {
            return true;
        }
        w.handle_error(ParseErrorCode::ValueExpected, &[], &[]);
        return false;
    }
    if !w.parse_value() {
        w.handle_error(ParseErrorCode::ValueExpected, &[], &[]);
        return false;
    }
    if w.s.token() != SyntaxKind::Eof {
        w.handle_error(ParseErrorCode::EndOfFileExpected, &[], &[]);
    }
    true
}

enum Parent {
    Root,
    Array(Vec<Value>),
    Object(Object),
}

struct ValueBuilder {
    stack: Vec<(Parent, Option<String>)>,
    cur: Parent,
    property: Option<String>,
    root: Option<Value>,
    errors: Vec<ParseError>,
}

impl ValueBuilder {
    fn on_value(&mut self, value: Value) {
        match &mut self.cur {
            Parent::Root => {
                if self.root.is_none() {
                    self.root = Some(value);
                }
            }
            Parent::Array(a) => a.push(value),
            Parent::Object(o) => {
                if let Some(p) = &self.property {
                    // `obj['__proto__'] = v` sets the prototype, not an own key.
                    if p != "__proto__" {
                        o.insert(p.clone(), value);
                    }
                }
            }
        }
    }

    fn begin(&mut self, next: Parent) {
        let prev = std::mem::replace(&mut self.cur, next);
        let property = self.property.take();
        self.stack.push((prev, property));
    }

    fn end(&mut self) {
        let Some((prev, property)) = self.stack.pop() else { return };
        let done = std::mem::replace(&mut self.cur, prev);
        self.property = property;
        let value = match done {
            Parent::Array(a) => Value::Array(a),
            Parent::Object(o) => Value::Object(o),
            Parent::Root => return,
        };
        self.on_value(value);
        // `currentProperty` is left as the nested scope's (`null`) after a
        // nested value; nothing observes it before the next property.
        self.property = None;
    }
}

impl<'s> Visitor<'s> for ValueBuilder {
    fn on_object_begin(&mut self, _: usize) {
        self.begin(Parent::Object(Object::new()));
    }
    fn on_object_property(&mut self, name: Cow<'s, str>, _: usize, _: usize) {
        self.property = Some(name.into_owned());
    }
    fn on_object_end(&mut self, _: usize, _: usize) {
        self.end();
    }
    fn on_array_begin(&mut self, _: usize) {
        self.begin(Parent::Array(Vec::new()));
    }
    fn on_array_end(&mut self, _: usize, _: usize) {
        self.end();
    }
    fn on_string_value(&mut self, value: Cow<'s, str>, _: usize, _: usize) {
        self.on_value(Value::String(value.into_owned()));
    }
    fn on_number_value(&mut self, value: f64, _: usize, _: usize) {
        self.on_value(Value::Number(value));
    }
    fn on_bool_value(&mut self, value: bool, _: usize, _: usize) {
        self.on_value(Value::Bool(value));
    }
    fn on_null_value(&mut self, _: usize, _: usize) {
        self.on_value(Value::Null);
    }
    fn on_error(&mut self, error: ParseErrorCode, offset: usize, length: usize) {
        self.errors.push(ParseError { error, offset: offset as u32, length: length as u32 });
    }
}

/// `parse(text, errors, options)`. Fault tolerant: `None` is `undefined`.
pub fn parse(text: &str, opts: ParseOptions) -> (Option<Value>, Vec<ParseError>) {
    let mut b = ValueBuilder {
        stack: Vec::new(),
        cur: Parent::Root,
        property: None,
        root: None,
        errors: Vec::new(),
    };
    visit(text, &mut b, opts);
    (b.root, b.errors)
}

/// `parseJSON(source)` from `json.ts` (non-strict): comments and trailing commas
/// allowed, empty content an error, syntax errors ignored. `None` is `undefined`.
pub fn parse_json(source: &str) -> Option<Value> {
    parse(source, THEME_CHECK_OPTS).0
}

/// `parseJSON(source, undefined, true)`: `Err` carries the first error.
pub fn parse_json_strict(source: &str) -> Result<Option<Value>, ParseError> {
    let (value, errors) = parse(source, THEME_CHECK_OPTS);
    match errors.first() {
        Some(e) => Err(*e),
        None => Ok(value),
    }
}

const THEME_CHECK_OPTS: ParseOptions =
    ParseOptions { disallow_comments: false, allow_trailing_comma: true, allow_empty_content: false };

/// A `parseTree` node.
#[derive(Clone, Debug)]
pub struct TreeNode<'s> {
    pub kind: TreeKind<'s>,
    pub offset: u32,
    pub length: u32,
    pub children: Vec<TreeNode<'s>>,
}

#[derive(Clone, Debug)]
pub enum TreeKind<'s> {
    Object,
    Array,
    Property,
    String(Cow<'s, str>),
    Number(f64),
    Bool(bool),
    Null,
}

struct TreeBuilder<'s> {
    // Open containers, the artificial root array at index 0.
    stack: Vec<TreeNode<'s>>,
    errors: Vec<ParseError>,
}

impl<'s> TreeBuilder<'s> {
    fn top(&mut self) -> &mut TreeNode<'s> {
        self.stack.last_mut().expect("root")
    }

    fn close_top(&mut self) {
        let node = self.stack.pop().expect("open node");
        self.top().children.push(node);
    }

    fn ensure_property_complete(&mut self, end: usize) {
        if matches!(self.top().kind, TreeKind::Property) {
            let t = self.top();
            t.length = end as u32 - t.offset;
            self.close_top();
        }
    }

    fn leaf(&mut self, kind: TreeKind<'s>, offset: usize, length: usize) {
        let node =
            TreeNode { kind, offset: offset as u32, length: length as u32, children: Vec::new() };
        self.top().children.push(node);
        self.ensure_property_complete(offset + length);
    }
}

impl<'s> Visitor<'s> for TreeBuilder<'s> {
    fn on_object_begin(&mut self, offset: usize) {
        self.stack.push(TreeNode {
            kind: TreeKind::Object,
            offset: offset as u32,
            length: 0,
            children: Vec::new(),
        });
    }
    fn on_object_property(&mut self, name: Cow<'s, str>, offset: usize, length: usize) {
        let key = TreeNode {
            kind: TreeKind::String(name),
            offset: offset as u32,
            length: length as u32,
            children: Vec::new(),
        };
        self.stack.push(TreeNode {
            kind: TreeKind::Property,
            offset: offset as u32,
            length: 0,
            children: vec![key],
        });
    }
    fn on_object_end(&mut self, offset: usize, length: usize) {
        self.ensure_property_complete(offset + length);
        let t = self.top();
        t.length = (offset + length) as u32 - t.offset;
        self.close_top();
        self.ensure_property_complete(offset + length);
    }
    fn on_array_begin(&mut self, offset: usize) {
        self.stack.push(TreeNode {
            kind: TreeKind::Array,
            offset: offset as u32,
            length: 0,
            children: Vec::new(),
        });
    }
    fn on_array_end(&mut self, offset: usize, length: usize) {
        let t = self.top();
        t.length = (offset + length) as u32 - t.offset;
        self.close_top();
        self.ensure_property_complete(offset + length);
    }
    fn on_string_value(&mut self, value: Cow<'s, str>, offset: usize, length: usize) {
        self.leaf(TreeKind::String(value), offset, length);
    }
    fn on_number_value(&mut self, value: f64, offset: usize, length: usize) {
        self.leaf(TreeKind::Number(value), offset, length);
    }
    fn on_bool_value(&mut self, value: bool, offset: usize, length: usize) {
        self.leaf(TreeKind::Bool(value), offset, length);
    }
    fn on_null_value(&mut self, offset: usize, length: usize) {
        self.leaf(TreeKind::Null, offset, length);
    }
    fn on_separator(&mut self, sep: char, offset: usize) {
        if sep == ',' {
            self.ensure_property_complete(offset);
        }
    }
    fn on_error(&mut self, error: ParseErrorCode, offset: usize, length: usize) {
        self.errors.push(ParseError { error, offset: offset as u32, length: length as u32 });
    }
}

/// `parseTree(text, errors, options)`.
pub fn parse_tree(text: &str, opts: ParseOptions) -> (Option<TreeNode<'_>>, Vec<ParseError>) {
    let mut b = TreeBuilder {
        stack: vec![TreeNode {
            kind: TreeKind::Array,
            offset: 0,
            length: 0,
            children: Vec::new(),
        }],
        errors: Vec::new(),
    };
    visit(text, &mut b, opts);
    // Unbalanced stacks cannot happen: every begin has its end, EOF included.
    let root = b.stack.pop().expect("root");
    (root.children.into_iter().next(), b.errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_values() {
        let (v, e) = parse(r#"{"a":[1,true,null,"x\n"],"b":{"c":-1.5e2}}"#, ParseOptions::default());
        assert!(e.is_empty());
        assert_eq!(v.unwrap().to_json(), r#"{"a":[1,true,null,"x\n"],"b":{"c":-150}}"#);
    }

    #[test]
    fn surrogate_pairs_combine() {
        let (v, _) = parse(r#""😀""#, ParseOptions::default());
        assert_eq!(v.unwrap().as_str(), Some("\u{1F600}"));
    }

    #[test]
    fn errors_match_upstream_shape() {
        let (v, e) = parse("{\"a\": 1,}", ParseOptions::default());
        assert_eq!(v.unwrap().to_json(), r#"{"a":1}"#);
        // `}` where a property name is expected.
        assert_eq!(e[0].error, ParseErrorCode::PropertyNameExpected);
        assert_eq!(e[0].offset, 8);
        let (_, e) = parse("", ParseOptions::default());
        assert_eq!(e[0].error, ParseErrorCode::ValueExpected);
        assert_eq!(parse_json("// c\n{\"a\":1,}").unwrap().to_json(), r#"{"a":1}"#);
    }

    #[test]
    fn tree_offsets() {
        let (t, e) = parse_tree(r#"{"key": "value"}"#, ParseOptions::default());
        assert!(e.is_empty());
        let t = t.unwrap();
        assert_eq!((t.offset, t.length), (0, 16));
        let p = &t.children[0];
        assert_eq!((p.offset, p.length), (1, 14));
        assert_eq!((p.children[1].offset, p.children[1].length), (8, 7));
    }
}
