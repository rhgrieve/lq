//! Source -> flat `Token` list. Modal, as upstream: Default, HtmlTag,
//! QuotedValue, LiquidTag, LiquidVariableOutput. Offsets are bytes.

use crate::js;
use memchr::{memchr, memchr2, memchr3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TokenType {
    Text,
    LiquidTagOpen,
    LiquidTagClose,
    LiquidVariableOutputOpen,
    LiquidVariableOutputClose,
    HtmlTagOpen,
    HtmlCloseTagOpen,
    HtmlTagClose,
    HtmlSelfClose,
    HtmlCommentOpen,
    HtmlCommentClose,
    HtmlDoctypeOpen,
    HtmlEquals,
    HtmlQuoteOpen,
    HtmlQuoteClose,
    YamlFrontmatter,
    EndOfInput,
}

impl TokenType {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenType::Text => "Text",
            TokenType::LiquidTagOpen => "LiquidTagOpen",
            TokenType::LiquidTagClose => "LiquidTagClose",
            TokenType::LiquidVariableOutputOpen => "LiquidVariableOutputOpen",
            TokenType::LiquidVariableOutputClose => "LiquidVariableOutputClose",
            TokenType::HtmlTagOpen => "HtmlTagOpen",
            TokenType::HtmlCloseTagOpen => "HtmlCloseTagOpen",
            TokenType::HtmlTagClose => "HtmlTagClose",
            TokenType::HtmlSelfClose => "HtmlSelfClose",
            TokenType::HtmlCommentOpen => "HtmlCommentOpen",
            TokenType::HtmlCommentClose => "HtmlCommentClose",
            TokenType::HtmlDoctypeOpen => "HtmlDoctypeOpen",
            TokenType::HtmlEquals => "HtmlEquals",
            TokenType::HtmlQuoteOpen => "HtmlQuoteOpen",
            TokenType::HtmlQuoteClose => "HtmlQuoteClose",
            TokenType::YamlFrontmatter => "YamlFrontmatter",
            TokenType::EndOfInput => "EndOfInput",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub token_type: TokenType,
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TokenizeOptions {
    /// Skip the document-start YAML frontmatter check.
    pub skip_frontmatter: bool,
    /// Begin inside an HTML quoted attribute value opened by this char.
    pub inside_quoted_attribute: Option<char>,
    /// Begin inside an open HTML tag's attribute list. Ignored when
    /// `inside_quoted_attribute` is set.
    pub inside_html_tag: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Default,
    HtmlTag,
    QuotedValue,
    LiquidTag,
    LiquidVariableOutput,
}

/// Curly quotes are directional, so each closes on its partner.
fn closing_quote_for(open: char) -> char {
    match open {
        '\u{201C}' => '\u{201D}',
        '\u{201D}' => '\u{201C}',
        '\u{2018}' => '\u{2019}',
        '\u{2019}' => '\u{2018}',
        c => c,
    }
}

pub fn tokenize(source: &str, options: TokenizeOptions) -> Vec<Token> {
    let mut tokens = Vec::with_capacity(source.len() / 16 + 8);
    tokenize_from(source, 0, options, &mut tokens);
    tokens
}

/// Tokenizes `source[offset..]`, appending to `tokens` with absolute offsets. The
/// equivalent of upstream's `tokenize(source.slice(offset))` plus rebasing.
pub(crate) fn tokenize_from(
    source: &str,
    offset: usize,
    options: TokenizeOptions,
    tokens: &mut Vec<Token>,
) {
    Tokenizer {
        source,
        bytes: source.as_bytes(),
        tokens,
        mode_stack: Vec::with_capacity(4),
        mode: Mode::Default,
        pos: offset,
        text_start: None,
        text_is_space: false,
        quote: ('\0', '\0'),
    }
    .run(options);
}

struct Tokenizer<'a> {
    source: &'a str,
    bytes: &'a [u8],
    tokens: &'a mut Vec<Token>,
    mode_stack: Vec<Mode>,
    mode: Mode,
    pos: usize,
    text_start: Option<usize>,
    text_is_space: bool,
    /// The open quote and its closing partner.
    quote: (char, char),
}

impl Tokenizer<'_> {
    #[inline]
    fn starts_with(&self, s: &[u8]) -> bool {
        self.bytes[self.pos..].starts_with(s)
    }

    #[inline]
    fn flush_text(&mut self) {
        if let Some(start) = self.text_start.take() {
            self.tokens.push(Token {
                token_type: TokenType::Text,
                start: start as u32,
                end: self.pos as u32,
            });
        }
    }

    #[inline]
    fn emit(&mut self, token_type: TokenType, length: usize) {
        self.flush_text();
        self.tokens.push(Token {
            token_type,
            start: self.pos as u32,
            end: (self.pos + length) as u32,
        });
        self.pos += length;
    }

    #[inline]
    fn push_mode(&mut self, next: Mode) {
        self.mode_stack.push(self.mode);
        self.mode = next;
    }

    #[inline]
    fn pop_mode(&mut self) {
        self.mode = self.mode_stack.pop().unwrap_or(Mode::Default);
    }

    #[inline]
    fn start_text(&mut self) {
        if self.text_start.is_none() {
            self.text_start = Some(self.pos);
        }
    }

    fn scan_liquid_open(&mut self) -> bool {
        let rest = &self.bytes[self.pos..];
        if rest.len() < 2 || rest[0] != b'{' {
            return false;
        }
        let dash = rest.get(2) == Some(&b'-');
        match rest[1] {
            b'{' => {
                self.emit(TokenType::LiquidVariableOutputOpen, if dash { 3 } else { 2 });
                self.push_mode(Mode::LiquidVariableOutput);
                true
            }
            b'%' => {
                self.emit(TokenType::LiquidTagOpen, if dash { 3 } else { 2 });
                self.push_mode(Mode::LiquidTag);
                true
            }
            _ => false,
        }
    }

    fn scan_frontmatter(&mut self) {
        if !(self.starts_with(b"---\n") || self.starts_with(b"---\r\n")) {
            return;
        }
        let bytes = self.bytes;
        let search_start = self.pos + memchr(b'\n', &bytes[self.pos..]).map_or(0, |i| i + 1);
        let Some(close) = memchr::memmem::find(&bytes[search_start..], b"\n---") else { return };
        let mut end = search_start + close + 4;
        if bytes.get(end) == Some(&b'\r') {
            end += 1;
        }
        if bytes.get(end) == Some(&b'\n') {
            end += 1;
        }
        self.tokens.push(Token {
            token_type: TokenType::YamlFrontmatter,
            start: self.pos as u32,
            end: end as u32,
        });
        self.pos = end;
    }

    /// Scans LiquidTag or LiquidVariableOutput mode for the closer, whose first
    /// byte is `%` or `}` with a `}` second. The sequential scan's first match of
    /// `-X}`/`X}` is the first `X}`, moved back one byte if a `-` at or after the
    /// scan start precedes it.
    #[inline]
    fn scan_liquid_close(&mut self, first: u8, close: TokenType) {
        let bytes = self.bytes;
        let mut k = self.pos;
        let mut misses = 0;
        let found = loop {
            // The closer is usually close by. When the first byte keeps missing
            // (a stray `{{` in a body full of `}`), search for the pair instead.
            if misses == 8 {
                break memchr::memmem::find(&bytes[k..], &[first, b'}']).map(|i| k + i);
            }
            match memchr(first, &bytes[k..]) {
                None => break None,
                Some(i) => {
                    let j = k + i;
                    if bytes.get(j + 1) == Some(&b'}') {
                        break Some(j);
                    }
                    k = j + 1;
                    misses += 1;
                }
            }
        };
        let Some(j) = found else {
            self.start_text();
            self.pos = bytes.len();
            return;
        };
        let (at, width) = if j > self.pos && bytes[j - 1] == b'-' { (j - 1, 3) } else { (j, 2) };
        if at > self.pos {
            self.start_text();
            self.pos = at;
        }
        self.emit(close, width);
        self.pop_mode();
    }

    fn run(mut self, options: TokenizeOptions) {
        if let Some(q) = options.inside_quoted_attribute {
            self.quote = (q, closing_quote_for(q));
            self.mode_stack.push(Mode::HtmlTag);
            self.mode = Mode::QuotedValue;
        } else if options.inside_html_tag {
            self.mode = Mode::HtmlTag;
        }

        if !options.skip_frontmatter {
            self.scan_frontmatter();
        }

        let len = self.bytes.len();
        while self.pos < len {
            match self.mode {
                Mode::LiquidTag => self.scan_liquid_close(b'%', TokenType::LiquidTagClose),
                Mode::LiquidVariableOutput => {
                    self.scan_liquid_close(b'}', TokenType::LiquidVariableOutputClose)
                }
                Mode::Default => self.step_default(),
                Mode::HtmlTag => self.step_html_tag(),
                Mode::QuotedValue => self.step_quoted_value(),
            }
        }

        self.flush_text();
        self.tokens.push(Token {
            token_type: TokenType::EndOfInput,
            start: len as u32,
            end: len as u32,
        });
    }

    #[inline]
    fn step_default(&mut self) {
        let bytes = self.bytes;
        match memchr3(b'{', b'<', b'-', &bytes[self.pos..]) {
            None => {
                self.start_text();
                self.pos = bytes.len();
                return;
            }
            Some(0) => {}
            Some(i) => {
                self.start_text();
                self.pos += i;
            }
        }
        match bytes[self.pos] {
            b'{' => {
                if self.scan_liquid_open() {
                    return;
                }
            }
            b'-' => {
                if self.starts_with(b"-->") {
                    self.emit(TokenType::HtmlCommentClose, 3);
                    return;
                }
            }
            _ => {
                if self.starts_with(b"<!--") {
                    self.emit(TokenType::HtmlCommentOpen, 4);
                    return;
                }
                if self.starts_with(b"<!") {
                    self.emit(TokenType::HtmlDoctypeOpen, 2);
                    self.push_mode(Mode::HtmlTag);
                    return;
                }
                if self.starts_with(b"</") {
                    if let Some(&after) = bytes.get(self.pos + 2) {
                        if after.is_ascii_alphabetic() || after == b'{' {
                            self.emit(TokenType::HtmlCloseTagOpen, 2);
                            self.push_mode(Mode::HtmlTag);
                            return;
                        }
                    }
                }
                if let Some(&after) = bytes.get(self.pos + 1) {
                    if after.is_ascii_alphabetic() || after == b'{' {
                        self.emit(TokenType::HtmlTagOpen, 1);
                        self.push_mode(Mode::HtmlTag);
                        return;
                    }
                }
            }
        }
        self.start_text();
        self.pos += 1;
    }

    #[inline]
    fn step_html_tag(&mut self) {
        let c = self.bytes[self.pos];
        let (width, is_space) = match c {
            b'{' => {
                if self.scan_liquid_open() {
                    return;
                }
                (1, false)
            }
            b'/' if self.bytes.get(self.pos + 1) == Some(&b'>') => {
                self.emit(TokenType::HtmlSelfClose, 2);
                self.pop_mode();
                return;
            }
            b'>' => {
                self.emit(TokenType::HtmlTagClose, 1);
                self.pop_mode();
                return;
            }
            b'=' => {
                self.emit(TokenType::HtmlEquals, 1);
                return;
            }
            b'"' | b'\'' => {
                self.open_quote(c as char, 1);
                return;
            }
            0..=0x7F => (1, js::is_ascii_ws(c)),
            _ => {
                let ch = self.source[self.pos..].chars().next().unwrap_or('\u{FFFD}');
                if matches!(ch, '\u{201C}' | '\u{201D}' | '\u{2018}' | '\u{2019}') {
                    self.open_quote(ch, ch.len_utf8());
                    return;
                }
                (ch.len_utf8(), js::is_ws_char(ch))
            }
        };
        if self.text_start.is_some() && is_space != self.text_is_space {
            self.flush_text();
        }
        if self.text_start.is_none() {
            self.text_is_space = is_space;
            self.text_start = Some(self.pos);
        }
        self.pos += width;
    }

    fn open_quote(&mut self, q: char, width: usize) {
        self.quote = (q, closing_quote_for(q));
        self.emit(TokenType::HtmlQuoteOpen, width);
        self.push_mode(Mode::QuotedValue);
    }

    #[inline]
    fn step_quoted_value(&mut self) {
        let bytes = self.bytes;
        let (open, close) = self.quote;
        // Every quote char's first byte: itself, or 0xE2 for the curly ones.
        let needle = if open.is_ascii() { open as u8 } else { 0xE2 };
        match memchr2(b'{', needle, &bytes[self.pos..]) {
            None => {
                self.start_text();
                self.pos = bytes.len();
                return;
            }
            Some(0) => {}
            Some(i) => {
                self.start_text();
                self.pos += i;
            }
        }
        if bytes[self.pos] == b'{' {
            if self.scan_liquid_open() {
                return;
            }
        } else if open.is_ascii() {
            self.emit(TokenType::HtmlQuoteClose, 1);
            self.pop_mode();
            return;
        } else {
            let ch = self.source[self.pos..].chars().next().unwrap_or('\u{FFFD}');
            if ch == open || ch == close {
                self.emit(TokenType::HtmlQuoteClose, ch.len_utf8());
                self.pop_mode();
                return;
            }
            self.start_text();
            self.pos += ch.len_utf8();
            return;
        }
        self.start_text();
        self.pos += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TokenType as T;

    fn types(source: &str) -> Vec<(TokenType, &str)> {
        tokenize(source, TokenizeOptions::default())
            .into_iter()
            .map(|t| (t.token_type, &source[t.start as usize..t.end as usize]))
            .collect()
    }

    #[test]
    fn whitespace_stripping_close_is_found_after_the_open_dash() {
        // The `-` belongs to the opener, so the closer is two bytes.
        assert_eq!(
            types("{%-%}"),
            [(T::LiquidTagOpen, "{%-"), (T::LiquidTagClose, "%}"), (T::EndOfInput, "")]
        );
        assert_eq!(
            types("{{ a -}}"),
            [
                (T::LiquidVariableOutputOpen, "{{"),
                (T::Text, " a "),
                (T::LiquidVariableOutputClose, "-}}"),
                (T::EndOfInput, ""),
            ]
        );
    }

    #[test]
    fn first_closer_wins_past_many_false_starts() {
        let source = format!("{{{{{}}}}}", "} ".repeat(50));
        let tokens = types(&source);
        assert_eq!(tokens[2].0, T::LiquidVariableOutputClose);
        assert_eq!(tokens[1].1.len(), 100);
    }

    #[test]
    fn curly_quotes_are_one_token_and_close_on_their_partner() {
        assert_eq!(
            types("<a b=\u{201C}c d\u{201D}>"),
            [
                (T::HtmlTagOpen, "<"),
                (T::Text, "a"),
                (T::Text, " "),
                (T::Text, "b"),
                (T::HtmlEquals, "="),
                (T::HtmlQuoteOpen, "\u{201C}"),
                (T::Text, "c d"),
                (T::HtmlQuoteClose, "\u{201D}"),
                (T::HtmlTagClose, ">"),
                (T::EndOfInput, ""),
            ]
        );
    }

    #[test]
    fn other_multibyte_in_a_curly_quoted_value_is_text() {
        // U+2014 shares its first byte with the curly quotes.
        let tokens = types("<a b=\u{201C}\u{2014}\u{201D}>");
        assert_eq!(tokens[6], (T::Text, "\u{2014}"));
        assert_eq!(tokens[7].0, T::HtmlQuoteClose);
    }

    #[test]
    fn js_whitespace_splits_tag_text_but_u0085_does_not() {
        let t = types("<a\u{A0}b\u{FEFF}c\u{85}d>");
        assert_eq!(
            t[1..7].iter().map(|x| x.1).collect::<Vec<_>>(),
            ["a", "\u{A0}", "b", "\u{FEFF}", "c\u{85}d", ">"]
        );
    }

    #[test]
    fn angle_bracket_needs_a_letter_or_brace() {
        assert_eq!(types("< a</ b<1")[0], (T::Text, "< a</ b<1"));
        assert_eq!(types("</{{ a }}>")[0].0, T::HtmlCloseTagOpen);
    }

    #[test]
    fn frontmatter_is_only_at_offset_zero_and_needs_its_close() {
        assert_eq!(types("---\na\n---\nb")[0], (T::YamlFrontmatter, "---\na\n---\n"));
        assert_eq!(types("---\n---\n")[0].0, T::Text);
        assert_eq!(types(" ---\na\n---\n")[0].0, T::Text);
        let tokens = tokenize(
            "---\na\n---\n",
            TokenizeOptions { skip_frontmatter: true, ..Default::default() },
        );
        assert_eq!(tokens[0].token_type, T::Text);
    }

    #[test]
    fn resuming_inside_a_quoted_attribute_closes_on_the_quote() {
        let source = "x\">y";
        let tokens = tokenize(
            source,
            TokenizeOptions {
                skip_frontmatter: true,
                inside_quoted_attribute: Some('"'),
                inside_html_tag: false,
            },
        );
        let kinds: Vec<_> = tokens.iter().map(|t| t.token_type).collect();
        assert_eq!(
            kinds,
            [T::Text, T::HtmlQuoteClose, T::HtmlTagClose, T::Text, T::EndOfInput]
        );
    }

    #[test]
    fn eof_token_sits_at_the_byte_length() {
        let tokens = tokenize("\u{E9}", TokenizeOptions::default());
        assert_eq!(tokens.last().map(|t| (t.start, t.end)), Some((2, 2)));
    }
}
