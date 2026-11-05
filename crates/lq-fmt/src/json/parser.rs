//! The expression subset of `@babel/parser` that Prettier's `json` parser accepts
//! (`createJsonParse` in `parser-babel`): whatever parses here is what Babel parses and
//! `assertJsonNode` lets through. Anything else is an error, which is all the embed flow
//! needs: Prettier then prints the block verbatim.

use std::borrow::Cow;

pub type Id = u32;

#[derive(Debug)]
pub enum Kind<'a> {
    Root { node: Id },
    Object { properties: Vec<Id> },
    Array { elements: Vec<Option<Id>> },
    Property { key: Id, value: Id },
    Unary { operator: &'static str, argument: Id },
    Null,
    Boolean(&'static str),
    Str(&'a str),
    Numeric(&'a str),
    Identifier(Cow<'a, str>),
    Template { raw: &'a str },
}

#[derive(Debug)]
pub struct JsonNode<'a> {
    pub kind: Kind<'a>,
    pub start: usize,
    pub end: usize,
    /// Indexes into `Ast::comments`, in attachment order.
    pub comments: Vec<u32>,
}

#[derive(Debug)]
pub struct Comment<'a> {
    pub block: bool,
    pub value: &'a str,
    pub start: usize,
    pub end: usize,
    pub leading: bool,
    pub trailing: bool,
}

#[derive(Debug)]
pub struct Ast<'a> {
    pub text: &'a str,
    pub nodes: Vec<JsonNode<'a>>,
    pub comments: Vec<Comment<'a>>,
    pub root: Id,
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    nodes: Vec<JsonNode<'a>>,
    comments: Vec<Comment<'a>>,
}

type R<T> = Result<T, ()>;

fn is_id_start(c: char) -> bool {
    c == '$' || c == '_' || unicode_ident::is_xid_start(c) || is_id_start_extra(c)
}

fn is_id_continue(c: char) -> bool {
    c == '$'
        || c == '\u{200C}'
        || c == '\u{200D}'
        || unicode_ident::is_xid_continue(c)
        || is_id_start_extra(c)
}

/// `Other_ID_Start` and friends that XID excludes but ES identifiers allow.
fn is_id_start_extra(c: char) -> bool {
    matches!(c, '\u{2118}' | '\u{212E}' | '\u{309B}' | '\u{309C}' | '\u{1885}' | '\u{1886}')
}

fn is_js_newline(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn is_babel_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{09}' | '\u{0B}' | '\u{0C}' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}'
            | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

impl<'a> Parser<'a> {
    #[inline]
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    #[inline]
    fn peek_at(&self, n: usize) -> Option<u8> {
        self.bytes.get(self.pos + n).copied()
    }

    #[inline]
    fn peek_char(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn line_comment(&mut self, prefix: usize) {
        let start = self.pos;
        let value_start = start + prefix;
        let rest = &self.text[value_start..];
        let len = rest.find(is_js_newline).unwrap_or(rest.len());
        self.pos = value_start + len;
        self.comments.push(Comment {
            block: false,
            value: &self.text[value_start..self.pos],
            start,
            end: self.pos,
            leading: false,
            trailing: false,
        });
    }

    /// Babel's `skipSpace`.
    fn skip_space(&mut self) -> R<()> {
        let space_start = self.pos;
        let mut newline_seen = false;
        while let Some(c) = self.peek_char() {
            match c {
                '/' => match self.peek_at(1) {
                    Some(b'*') => {
                        let start = self.pos;
                        let rest = &self.text[start + 2..];
                        let Some(close) = rest.find("*/") else {
                            return Err(());
                        };
                        let value = &rest[..close];
                        newline_seen |= value.contains(is_js_newline);
                        self.pos = start + 2 + close + 2;
                        self.comments.push(Comment {
                            block: true,
                            value,
                            start,
                            end: self.pos,
                            leading: false,
                            trailing: false,
                        });
                    }
                    Some(b'/') => self.line_comment(2),
                    _ => return Ok(()),
                },
                '<' if self.text[self.pos..].starts_with("<!--") => self.line_comment(4),
                '-' if self.text[self.pos..].starts_with("-->")
                    && (space_start == 0 || newline_seen) =>
                {
                    self.line_comment(3);
                }
                c if is_js_newline(c) => {
                    newline_seen = true;
                    self.pos += c.len_utf8();
                }
                c if is_babel_whitespace(c) => self.pos += c.len_utf8(),
                _ => return Ok(()),
            }
        }
        Ok(())
    }

    fn push(&mut self, kind: Kind<'a>, start: usize, end: usize) -> Id {
        self.nodes.push(JsonNode { kind, start, end, comments: Vec::new() });
        (self.nodes.len() - 1) as Id
    }

    fn hex(&mut self, count: usize) -> R<u32> {
        let mut v = 0u32;
        for _ in 0..count {
            let d = (self.peek().ok_or(())? as char).to_digit(16).ok_or(())?;
            v = v * 16 + d;
            self.pos += 1;
        }
        Ok(v)
    }

    /// `\u` escape after the `\u` has been consumed.
    fn unicode_escape(&mut self) -> R<u32> {
        if self.peek() == Some(b'{') {
            self.pos += 1;
            let mut v = 0u32;
            let mut digits = 0;
            while let Some(d) = self.peek().and_then(|b| (b as char).to_digit(16)) {
                v = v * 16 + d;
                if v > 0x10FFFF {
                    return Err(());
                }
                digits += 1;
                self.pos += 1;
            }
            if digits == 0 || self.peek() != Some(b'}') {
                return Err(());
            }
            self.pos += 1;
            Ok(v)
        } else {
            self.hex(4)
        }
    }

    /// The escape following a backslash in a string. `in_template` rejects what an
    /// untagged template rejects.
    fn escape(&mut self, in_template: bool) -> R<()> {
        let c = self.peek_char().ok_or(())?;
        self.pos += c.len_utf8();
        match c {
            'x' => {
                self.hex(2)?;
            }
            'u' => {
                self.unicode_escape()?;
            }
            // `\0` not followed by a digit is the only valid digit escape.
            '0'..='9'
                if in_template
                    && (c != '0' || self.peek().is_some_and(|b| b.is_ascii_digit())) =>
            {
                return Err(());
            }
            _ => {}
        }
        Ok(())
    }

    fn string(&mut self) -> R<(&'a str, usize, usize)> {
        let start = self.pos;
        let quote = self.bytes[start];
        self.pos += 1;
        loop {
            let c = self.peek_char().ok_or(())?;
            self.pos += c.len_utf8();
            match c {
                '\n' | '\r' => return Err(()),
                '\\' => self.escape(false)?,
                c if c as u32 == quote as u32 => break,
                _ => {}
            }
        }
        Ok((&self.text[start..self.pos], start, self.pos))
    }

    fn template(&mut self) -> R<Id> {
        let start = self.pos;
        self.pos += 1;
        let raw_start = self.pos;
        loop {
            let c = self.peek_char().ok_or(())?;
            match c {
                '`' => break,
                '$' if self.peek_at(1) == Some(b'{') => return Err(()),
                '\\' => {
                    self.pos += 1;
                    self.escape(true)?;
                }
                c => self.pos += c.len_utf8(),
            }
        }
        let raw = &self.text[raw_start..self.pos];
        self.pos += 1;
        Ok(self.push(Kind::Template { raw }, start, self.pos))
    }

    /// Babel's `readInt`: digits with separators; `None` when there are no digits.
    fn read_int(&mut self, radix: u32) -> R<Option<()>> {
        let start = self.pos;
        let mut last_was_sep = false;
        while let Some(b) = self.peek() {
            if b == b'_' {
                if self.pos == start || last_was_sep {
                    return Err(());
                }
                last_was_sep = true;
                self.pos += 1;
                continue;
            }
            if (b as char).to_digit(radix).is_none() {
                break;
            }
            last_was_sep = false;
            self.pos += 1;
        }
        if last_was_sep {
            return Err(());
        }
        Ok(if self.pos == start { None } else { Some(()) })
    }

    fn number(&mut self) -> R<(&'a str, usize, usize)> {
        let start = self.pos;
        let first = self.bytes[start];
        let mut is_bigint = false;
        if first == b'0' && matches!(self.peek_at(1), Some(b'x' | b'X' | b'o' | b'O' | b'b' | b'B')) {
            let radix = match self.bytes[start + 1] {
                b'x' | b'X' => 16,
                b'o' | b'O' => 8,
                _ => 2,
            };
            self.pos += 2;
            self.read_int(radix)?.ok_or(())?;
            if self.peek() == Some(b'n') {
                is_bigint = true;
                self.pos += 1;
            }
        } else {
            let starts_with_dot = first == b'.';
            let mut octal = false;
            if !starts_with_dot {
                self.read_int(10)?.ok_or(())?;
                let integer = &self.text[start..self.pos];
                if integer.len() >= 2 && first == b'0' {
                    if integer.contains('_') {
                        return Err(());
                    }
                    octal = !integer.contains(['8', '9']);
                }
            }
            if self.peek() == Some(b'.') && !octal {
                self.pos += 1;
                self.read_int(10)?;
            }
            if matches!(self.peek(), Some(b'e' | b'E')) && !octal {
                self.pos += 1;
                if matches!(self.peek(), Some(b'+' | b'-')) {
                    self.pos += 1;
                }
                self.read_int(10)?.ok_or(())?;
            }
            if self.peek() == Some(b'n') {
                is_bigint = true;
                self.pos += 1;
            }
        }
        if is_bigint {
            // `BigIntLiteral` is not allowed in JSON.
            return Err(());
        }
        if self.peek_char().is_some_and(is_id_start) || self.peek() == Some(b'\\') {
            return Err(());
        }
        Ok((&self.text[start..self.pos], start, self.pos))
    }

    /// An IdentifierName; returns the cooked name and whether it held an escape.
    fn identifier_name(&mut self) -> R<(Cow<'a, str>, bool, usize, usize)> {
        let start = self.pos;
        let mut cooked: Option<String> = None;
        let mut first = true;
        while let Some(c) = self.peek_char() {
            if c == '\\' {
                let before = self.pos;
                if self.peek_at(1) != Some(b'u') {
                    return Err(());
                }
                self.pos += 2;
                let v = self.unicode_escape()?;
                let ch = char::from_u32(v).ok_or(())?;
                if !(if first { is_id_start(ch) } else { is_id_continue(ch) }) {
                    return Err(());
                }
                cooked.get_or_insert_with(|| self.text[start..before].to_string()).push(ch);
            } else if if first { is_id_start(c) } else { is_id_continue(c) } {
                self.pos += c.len_utf8();
                if let Some(s) = cooked.as_mut() {
                    s.push(c);
                }
            } else {
                break;
            }
            first = false;
        }
        if first {
            return Err(());
        }
        let escaped = cooked.is_some();
        let name = match cooked {
            Some(s) => Cow::Owned(s),
            None => Cow::Borrowed(&self.text[start..self.pos]),
        };
        Ok((name, escaped, start, self.pos))
    }

    fn value(&mut self) -> R<Id> {
        let c = self.peek_char().ok_or(())?;
        match c {
            '{' => self.object(),
            '[' => self.array(),
            '"' | '\'' => {
                let (raw, s, e) = self.string()?;
                Ok(self.push(Kind::Str(raw), s, e))
            }
            '`' => self.template(),
            '0'..='9' => {
                let (raw, s, e) = self.number()?;
                Ok(self.push(Kind::Numeric(raw), s, e))
            }
            '.' if self.peek_at(1).is_some_and(|b| b.is_ascii_digit()) => {
                let (raw, s, e) = self.number()?;
                Ok(self.push(Kind::Numeric(raw), s, e))
            }
            '+' | '-' => self.unary(c),
            _ => {
                let (name, escaped, s, e) = self.identifier_name()?;
                let kind = match &*name {
                    "null" if !escaped => Kind::Null,
                    "true" if !escaped => Kind::Boolean("true"),
                    "false" if !escaped => Kind::Boolean("false"),
                    "Infinity" | "NaN" | "undefined" => Kind::Identifier(name),
                    _ => return Err(()),
                };
                Ok(self.push(kind, s, e))
            }
        }
    }

    fn unary(&mut self, c: char) -> R<Id> {
        let start = self.pos;
        self.pos += 1;
        if self.peek() == Some(c as u8) || self.peek() == Some(b'=') {
            return Err(());
        }
        self.skip_space()?;
        let argument = match self.peek_char().ok_or(())? {
            '0'..='9' => {
                let (raw, s, e) = self.number()?;
                self.push(Kind::Numeric(raw), s, e)
            }
            '.' if self.peek_at(1).is_some_and(|b| b.is_ascii_digit()) => {
                let (raw, s, e) = self.number()?;
                self.push(Kind::Numeric(raw), s, e)
            }
            _ => {
                let (name, _, s, e) = self.identifier_name()?;
                if &*name != "Infinity" && &*name != "NaN" {
                    return Err(());
                }
                self.push(Kind::Identifier(name), s, e)
            }
        };
        let end = self.nodes[argument as usize].end;
        let operator = if c == '+' { "+" } else { "-" };
        Ok(self.push(Kind::Unary { operator, argument }, start, end))
    }

    fn array(&mut self) -> R<Id> {
        let start = self.pos;
        self.pos += 1;
        let mut elements = Vec::new();
        loop {
            self.skip_space()?;
            match self.peek() {
                Some(b']') => break,
                Some(b',') => {
                    self.pos += 1;
                    elements.push(None);
                    continue;
                }
                None => return Err(()),
                _ => {}
            }
            let v = self.value()?;
            elements.push(Some(v));
            self.skip_space()?;
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => break,
                _ => return Err(()),
            }
        }
        self.pos += 1;
        Ok(self.push(Kind::Array { elements }, start, self.pos))
    }

    fn object(&mut self) -> R<Id> {
        let start = self.pos;
        self.pos += 1;
        let mut properties = Vec::new();
        let mut proto_used = false;
        loop {
            self.skip_space()?;
            match self.peek() {
                Some(b'}') => break,
                None => return Err(()),
                _ => {}
            }
            let (property, is_proto) = self.property()?;
            if is_proto {
                if proto_used {
                    return Err(());
                }
                proto_used = true;
            }
            properties.push(property);
            self.skip_space()?;
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => break,
                _ => return Err(()),
            }
        }
        self.pos += 1;
        Ok(self.push(Kind::Object { properties }, start, self.pos))
    }

    fn property(&mut self) -> R<(Id, bool)> {
        let start = self.pos;
        let mut is_proto = false;
        let key = match self.peek_char().ok_or(())? {
            '"' | '\'' => {
                let (raw, s, e) = self.string()?;
                is_proto = string_value_is_proto(raw);
                self.push(Kind::Str(raw), s, e)
            }
            '0'..='9' => {
                let (raw, s, e) = self.number()?;
                self.push(Kind::Numeric(raw), s, e)
            }
            '.' if self.peek_at(1).is_some_and(|b| b.is_ascii_digit()) => {
                let (raw, s, e) = self.number()?;
                self.push(Kind::Numeric(raw), s, e)
            }
            _ => {
                let (name, _, s, e) = self.identifier_name()?;
                is_proto = &*name == "__proto__";
                self.push(Kind::Identifier(name), s, e)
            }
        };
        self.skip_space()?;
        if self.peek() != Some(b':') {
            return Err(());
        }
        self.pos += 1;
        self.skip_space()?;
        let value = self.value()?;
        let end = self.nodes[value as usize].end;
        Ok((self.push(Kind::Property { key, value }, start, end), is_proto))
    }
}

/// Whether a string literal's value is `__proto__`, cooking any escapes.
fn string_value_is_proto(raw: &str) -> bool {
    let inner = &raw[1..raw.len() - 1];
    if !inner.contains('\\') {
        return inner == "__proto__";
    }
    let mut out = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('x') => {
                let h: String = chars.by_ref().take(2).collect();
                out.extend(u32::from_str_radix(&h, 16).ok().and_then(char::from_u32));
            }
            Some('u') => {
                let h: String = if chars.peek() == Some(&'{') {
                    chars.next();
                    chars.by_ref().take_while(|c| *c != '}').collect()
                } else {
                    chars.by_ref().take(4).collect()
                };
                out.extend(u32::from_str_radix(&h, 16).ok().and_then(char::from_u32));
            }
            Some('\n') | Some('\r') | Some('\u{2028}') | Some('\u{2029}') => {}
            Some(other) => out.push(other),
            None => {}
        }
    }
    out == "__proto__"
}

/// `createJsonParse()(text)`. `Err(())` is any parse or `assertJsonNode` failure: which
/// one does not matter, as Prettier's embed flow discards the error.
#[allow(clippy::result_unit_err)]
pub fn parse(text: &str) -> Result<Ast<'_>, ()> {
    let mut p = Parser { text, bytes: text.as_bytes(), pos: 0, nodes: Vec::new(), comments: Vec::new() };
    p.skip_space()?;
    let node = p.value()?;
    p.skip_space()?;
    if p.pos != text.len() {
        return Err(());
    }
    let root = p.push(Kind::Root { node }, 0, text.len());
    Ok(Ast { text, nodes: p.nodes, comments: p.comments, root })
}
