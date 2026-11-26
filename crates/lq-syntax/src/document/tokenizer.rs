//! Source -> flat `Token` list. Modal, as upstream: Default, HtmlTag,
//! QuotedValue, LiquidTag, LiquidVariableOutput, and for `ParseTarget::Main`
//! also HtmlComment and RawText. Offsets are bytes.

use super::raw_body::{self, find_raw_body_end, RawBodyEnd};
use crate::js;
use crate::ParseTarget;
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
    /// The parser the tokens are for. The fields below only apply to `Main`.
    pub target: ParseTarget,
    /// `insideHtmlComment`: begin inside an HTML comment's body.
    pub inside_html_comment: bool,
    /// `rawText`: tokenize the whole input as the body of a `<script>` or
    /// `<style>` element, where only Liquid delimiters are tokens.
    pub raw_text: bool,
    /// `rawTags`: a raw tag's body is opaque. It ends where `findRawBodyEnd`
    /// ends it, only Liquid delimiters are tokens inside it, and the mode before
    /// the tag holds again after it (5aec106f).
    pub raw_tags: bool,
    /// `builtinRawTags`: the AST scan, which skips literal bodies (`raw`,
    /// `comment`, `doc`, `schema`) and records where each body ends.
    pub builtin_raw_tags: bool,
    /// `parseHtml: false`.
    pub liquid_only: bool,
}

impl TokenizeOptions {
    /// `tokenize(source)` with upstream's default options, as `target` does it.
    pub fn for_target(target: ParseTarget) -> Self {
        TokenizeOptions { target, ..TokenizeOptions::default() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Default,
    HtmlTag,
    QuotedValue,
    LiquidTag,
    LiquidVariableOutput,
    RawText,
    HtmlComment,
}

/// The elements whose open tag's `>` changes the tokenizer's state on `main`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Element {
    Svg,
    Script,
    Style,
}

impl Element {
    fn name(self) -> &'static str {
        match self {
            Element::Svg => "svg",
            Element::Script => "script",
            Element::Style => "style",
        }
    }
}

/// A raw tag body being tokenized, and the state to resume after it.
#[derive(Clone, Copy)]
struct RawBody {
    tag_start: usize,
    mode: Mode,
    depth: usize,
    quote: (char, char),
    raw_text_tag: Option<Element>,
}

/// `[a-zA-Z{]`: what may follow `<` or `</` to open an HTML tag.
#[inline]
fn is_tag_name_start(b: Option<&u8>) -> bool {
    b.is_some_and(|&b| b.is_ascii_alphabetic() || b == b'{')
}

/// `BUILTIN_RAW_OPEN` at `at`: `/\{%-?\s*(raw|comment|doc|schema|javascript|stylesheet|style)(?=\s|-?%})/`.
fn builtin_raw_open_at(source: &str, at: usize) -> Option<&'static str> {
    let b = source.as_bytes();
    if !b[at..].starts_with(b"{%") {
        return None;
    }
    let mut i = at + 2;
    if b.get(i) == Some(&b'-') {
        i += 1;
    }
    loop {
        let w = js::ws_width_at(source, i);
        if w == 0 {
            break;
        }
        i += w;
    }
    const NAMES: [&str; 7] = ["raw", "comment", "doc", "schema", "javascript", "stylesheet", "style"];
    let name = NAMES.into_iter().find(|n| b[i..].starts_with(n.as_bytes()))?;
    let after = i + name.len();
    let rest = &b[after..];
    (js::ws_width_at(source, after) > 0 || rest.starts_with(b"%}") || rest.starts_with(b"-%}"))
        .then_some(name)
}

/// The tokens and, on `main` with `builtin_raw_tags`, where each raw tag's body
/// ends, by the index of the tag's `LiquidTagClose` (`rawBodyEnds`).
pub(crate) fn tokenize_document(
    source: &str,
    options: TokenizeOptions,
) -> (Vec<Token>, Vec<(usize, RawBodyEnd)>) {
    let mut tokens = Vec::with_capacity(source.len() / 16 + 8);
    let ends = run_tokenizer(source, 0, options, &mut tokens);
    (tokens, ends)
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
    run_tokenizer(source, offset, options, tokens);
}

fn run_tokenizer(
    source: &str,
    offset: usize,
    options: TokenizeOptions,
    tokens: &mut Vec<Token>,
) -> Vec<(usize, RawBodyEnd)> {
    let main = options.target.is_main();
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
        main,
        lex_end: source.len(),
        raw_tags: main && options.raw_tags,
        builtin_raw_tags: main && options.raw_tags && options.builtin_raw_tags,
        liquid_only: options.liquid_only,
        raw_bodies: Vec::new(),
        literal_body_end: None,
        unresolved: Vec::new(),
        ends: Vec::new(),
        comment_body_start: None,
        liquid_tag_start: 0,
        inside_doctype: false,
        in_svg: 0,
        pending_element: None,
        raw_text_tag: None,
    }
    .run(options)
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

    // `ParseTarget::Main` only.
    main: bool,
    /// `lexicalSource.length`: the end of the raw body being tokenized, or of
    /// the source.
    lex_end: usize,
    raw_tags: bool,
    builtin_raw_tags: bool,
    liquid_only: bool,
    raw_bodies: Vec<RawBody>,
    /// Literal bodies cannot change HTML or attribute context, nor hold a raw tag.
    literal_body_end: Option<usize>,
    /// `unresolvedRawTags`: raw tags whose body did not end before a bound.
    unresolved: Vec<(&'a str, usize)>,
    ends: Vec<(usize, RawBodyEnd)>,
    /// Where the current comment's body starts, for the spec's empty comments
    /// `<!-->` and `<!--->`, which close only right after `<!--`.
    comment_body_start: Option<usize>,
    /// Where the `{%` of the Liquid tag being tokenized starts.
    liquid_tag_start: usize,
    inside_doctype: bool,
    /// SVG bodies keep markup tokens, including nested SVG elements.
    in_svg: u32,
    /// The element whose open tag is being tokenized, when it matters.
    pending_element: Option<Element>,
    /// The `<script>` or `<style>` whose body is being tokenized in RawText mode.
    raw_text_tag: Option<Element>,
}

impl<'a> Tokenizer<'a> {
    /// `match(s)`, which reads `lexicalSource`.
    #[inline]
    fn starts_with(&self, s: &[u8]) -> bool {
        self.bytes[self.pos..self.lex_end].starts_with(s)
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
        let rest = &self.bytes[self.pos..self.lex_end];
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
                self.liquid_tag_start = self.pos;
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
        let bytes = &self.bytes[..self.lex_end];
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
        if self.main && close == TokenType::LiquidTagClose {
            self.skip_raw_body();
        }
    }

    /// `skipRawBody`: at the close of a raw tag, bound its body once, keeping the
    /// surrounding mode for after it.
    fn skip_raw_body(&mut self) {
        if !self.raw_tags
            || (self.inside_doctype && !self.liquid_only)
            || self.literal_body_end.is_some_and(|end| self.pos <= end)
        {
            return;
        }
        let source: &'a str = self.source;
        let name: Option<&'a str> = if self.builtin_raw_tags {
            builtin_raw_open_at(source, self.liquid_tag_start)
        } else {
            let open = self.liquid_tag_start;
            let dash = self.bytes.get(open + 2) == Some(&b'-');
            let close = self.tokens[self.tokens.len() - 1].start as usize;
            let text = js::trim(js::slice(source, (open + if dash { 3 } else { 2 }) as u32, close as u32));
            let space = text.char_indices().find(|&(_, c)| js::is_ws_char(c)).map_or(text.len(), |(i, _)| i);
            Some(&text[..space])
        };
        let Some(name) = name else { return };
        let Some(rule) = raw_body::rule_for(name) else { return };
        let bound = self.lex_end;
        if self.unresolved.iter().any(|&(n, b)| n == name && b == bound) {
            return;
        }
        let Some(end) = find_raw_body_end(&source[..bound], self.pos, name, rule, None) else {
            // A tag of this name searched to this bound once isn't searched again.
            self.unresolved.push((name, bound));
            return;
        };
        let literal = !raw_body::parses_liquid_in_body(name);
        if self.builtin_raw_tags {
            self.ends.push((self.tokens.len() - 1, end));
            if literal {
                self.pos = end.tag_end as usize;
                return;
            }
        }
        let tag_start = end.tag_start as usize;
        // Plain bodies and one unfinished output need no additional lexer mode.
        let bytes = self.bytes;
        let first = memchr::memchr_iter(b'{', &bytes[self.pos..])
            .map(|i| self.pos + i)
            .find(|&i| matches!(bytes.get(i + 1), Some(b'{') | Some(b'%')));
        let Some(first) = first.filter(|&f| f + 2 <= tag_start) else {
            if self.pos < tag_start {
                self.emit(TokenType::Text, tag_start - self.pos);
            }
            return;
        };
        let open_end = first + if first + 2 < tag_start && bytes[first + 2] == b'-' { 3 } else { 2 };
        if bytes[first + 1] == b'{' && memchr::memmem::find(&bytes[open_end..tag_start], b"}}").is_none() {
            if self.pos < first {
                self.emit(TokenType::Text, first - self.pos);
            }
            self.emit(TokenType::LiquidVariableOutputOpen, open_end - first);
            if self.pos < tag_start {
                self.emit(TokenType::Text, tag_start - self.pos);
            }
            return;
        }
        self.raw_bodies.push(RawBody {
            tag_start,
            mode: self.mode,
            depth: self.mode_stack.len(),
            quote: self.quote,
            raw_text_tag: self.raw_text_tag,
        });
        if literal {
            self.literal_body_end = Some(tag_start);
        }
        self.lex_end = tag_start;
        self.mode = Mode::RawText;
        self.raw_text_tag = None;
    }

    /// `resumeRawBody`: past the end of a raw body, resume the mode before it.
    fn resume_raw_body(&mut self) -> bool {
        if self.raw_bodies.is_empty() {
            return false;
        }
        while self.pos >= self.lex_end {
            let Some(body) = self.raw_bodies.pop() else { return false };
            self.flush_text();
            self.mode = body.mode;
            self.mode_stack.truncate(body.depth);
            self.quote = body.quote;
            self.raw_text_tag = body.raw_text_tag;
            self.lex_end = self.raw_bodies.last().map_or(self.bytes.len(), |p| p.tag_start);
        }
        true
    }

    /// `elementNameAt`: the lowercase name at `at` when it is `svg`, `script` or
    /// `style` followed by whitespace, `/` or `>` (or the end).
    fn element_name_at(&self, at: usize) -> Option<Element> {
        let b = &self.bytes[at..];
        for el in [Element::Svg, Element::Script, Element::Style] {
            let n = el.name().len();
            if b.len() >= n && b[..n].eq_ignore_ascii_case(el.name().as_bytes()) {
                match self.source[at + n..].chars().next() {
                    None => return Some(el),
                    Some(c) if c == '/' || c == '>' || js::is_ws_char(c) => return Some(el),
                    _ => break,
                }
            }
        }
        // An `<svg` glued to Liquid may be the `svg` element with a Liquid
        // attribute or a compound name; only the parser can tell. Treat it as an
        // `<svg>`: in one, comments tokenize as they did before comment mode.
        (b.len() >= 4 && b[..3].eq_ignore_ascii_case(b"svg") && b[3] == b'{').then_some(Element::Svg)
    }

    /// `closesSvg`: `/^<\/svg\s*>/i` on the 64 code units at `at`.
    fn closes_svg(&self, at: usize) -> bool {
        let b = &self.bytes[at..];
        if b.len() < 5 || !b[2..5].eq_ignore_ascii_case(b"svg") {
            return false;
        }
        let mut units = 5;
        for c in self.source[at + 5..].chars() {
            units += c.len_utf16();
            if units > 64 {
                return false;
            }
            if c == '>' {
                return true;
            }
            if !js::is_ws_char(c) {
                return false;
            }
        }
        false
    }

    /// `closesRawText`: whether `</` at `at` begins the close tag of the current
    /// raw text element, whitespace allowed before `>`.
    fn closes_raw_text(&self, at: usize) -> bool {
        let Some(tag) = self.raw_text_tag else { return false };
        let name = tag.name().as_bytes();
        let b = &self.bytes[at + 2..];
        if b.len() < name.len() || !b[..name.len()].eq_ignore_ascii_case(name) {
            return false;
        }
        let rest = &self.source[at + 2 + name.len()..];
        rest.chars().find(|&c| !js::is_ws_char(c)) == Some('>')
    }

    /// `openPendingElement`: at the `>` of an open tag, enter the element it
    /// named. An `<svg>` starts an SVG body; a `<script>` or `<style>` outside
    /// SVG has a raw text body.
    fn open_pending_element(&mut self) {
        match self.pending_element.take() {
            Some(Element::Svg) => self.in_svg += 1,
            Some(el) if self.in_svg == 0 && self.mode == Mode::Default => {
                self.raw_text_tag = Some(el);
                self.push_mode(Mode::RawText);
            }
            _ => {}
        }
    }

    fn run(mut self, options: TokenizeOptions) -> Vec<(usize, RawBodyEnd)> {
        if self.main && options.inside_html_comment {
            self.mode = Mode::HtmlComment;
        } else if let Some(q) = options.inside_quoted_attribute {
            self.quote = (q, closing_quote_for(q));
            self.mode_stack.push(Mode::HtmlTag);
            self.mode = Mode::QuotedValue;
        } else if options.inside_html_tag {
            self.mode = Mode::HtmlTag;
        } else if self.main && options.raw_text {
            self.mode = Mode::RawText;
        }

        if !options.skip_frontmatter {
            self.scan_frontmatter();
        }

        let len = self.bytes.len();
        while self.pos < self.lex_end || (self.main && self.resume_raw_body()) {
            match self.mode {
                Mode::LiquidTag => self.scan_liquid_close(b'%', TokenType::LiquidTagClose),
                Mode::LiquidVariableOutput => {
                    self.scan_liquid_close(b'}', TokenType::LiquidVariableOutputClose)
                }
                Mode::Default if self.main => self.step_default_main(),
                Mode::Default => self.step_default(),
                Mode::HtmlTag => self.step_html_tag(),
                Mode::QuotedValue => self.step_quoted_value(),
                Mode::HtmlComment => self.step_html_comment(),
                Mode::RawText => self.step_raw_text(),
            }
        }

        self.flush_text();
        self.tokens.push(Token {
            token_type: TokenType::EndOfInput,
            start: len as u32,
            end: len as u32,
        });
        self.ends
    }

    /// Default mode on `main`: `<!--` opens HtmlComment mode, except inside an
    /// `<svg>`, where `<!--` and `-->` are the one-off tokens they were before
    /// (f5e177e6); and the `>` of a `<script>` or `<style>` opens RawText mode.
    fn step_default_main(&mut self) {
        let bytes = self.bytes;
        match memchr3(b'{', b'<', b'-', &bytes[self.pos..self.lex_end]) {
            None => {
                self.start_text();
                self.pos = self.lex_end;
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
                if self.in_svg > 0 && self.starts_with(b"-->") {
                    self.emit(TokenType::HtmlCommentClose, 3);
                    return;
                }
            }
            _ => {
                if self.starts_with(b"<!--") {
                    self.emit(TokenType::HtmlCommentOpen, 4);
                    if self.in_svg == 0 {
                        self.push_mode(Mode::HtmlComment);
                        self.comment_body_start = Some(self.pos);
                    }
                    return;
                }
                if self.starts_with(b"<!") {
                    self.inside_doctype = true;
                    self.emit(TokenType::HtmlDoctypeOpen, 2);
                    self.push_mode(Mode::HtmlTag);
                    return;
                }
                if self.starts_with(b"</") && is_tag_name_start(bytes.get(self.pos + 2)) {
                    if self.in_svg > 0 && self.closes_svg(self.pos) {
                        self.in_svg -= 1;
                    }
                    self.emit(TokenType::HtmlCloseTagOpen, 2);
                    self.push_mode(Mode::HtmlTag);
                    return;
                }
                if is_tag_name_start(bytes.get(self.pos + 1)) {
                    self.pending_element = self.element_name_at(self.pos + 1);
                    self.emit(TokenType::HtmlTagOpen, 1);
                    self.push_mode(Mode::HtmlTag);
                    return;
                }
            }
        }
        self.start_text();
        self.pos += 1;
    }

    /// HtmlComment mode (f5e177e6): a comment's body is text, except for Liquid,
    /// which still renders inside it, and the comment's close.
    fn step_html_comment(&mut self) {
        let bytes = self.bytes;
        let c = bytes[self.pos];
        if c == b'{' && self.scan_liquid_open() {
            return;
        }
        // The HTML spec closes `<!-->` and `<!--->` as empty comments
        // ("abrupt-closing-of-empty-comment"), only right after `<!--`.
        if self.comment_body_start == Some(self.pos) {
            if c == b'>' {
                self.emit(TokenType::HtmlCommentClose, 1);
                self.pop_mode();
                return;
            }
            if self.starts_with(b"->") {
                self.emit(TokenType::HtmlCommentClose, 2);
                self.pop_mode();
                return;
            }
        }
        if c == b'-' {
            if self.starts_with(b"-->") {
                self.emit(TokenType::HtmlCommentClose, 3);
                self.pop_mode();
                return;
            }
            // `--!>` closes a comment too ("incorrectly-closed-comment").
            if self.starts_with(b"--!>") {
                self.emit(TokenType::HtmlCommentClose, 4);
                self.pop_mode();
                return;
            }
        }
        self.start_text();
        self.pos += 1;
        self.pos += memchr2(b'{', b'-', &bytes[self.pos..self.lex_end]).unwrap_or(self.lex_end - self.pos);
    }

    /// RawText mode (64e4ffb3): a `<script>` or `<style>` body is text, except
    /// for Liquid and the element's close tag; so is a bounded raw tag body.
    fn step_raw_text(&mut self) {
        let bytes = self.bytes;
        let c = bytes[self.pos];
        if c == b'{' && self.scan_liquid_open() {
            return;
        }
        if c == b'<' && self.starts_with(b"</") && self.closes_raw_text(self.pos) {
            self.pop_mode();
            self.raw_text_tag = None;
            self.emit(TokenType::HtmlCloseTagOpen, 2);
            self.push_mode(Mode::HtmlTag);
            return;
        }
        self.start_text();
        self.pos += 1;
        self.pos += memchr2(b'{', b'<', &bytes[self.pos..self.lex_end]).unwrap_or(self.lex_end - self.pos);
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
                self.pending_element = None;
                return;
            }
            b'>' => {
                self.emit(TokenType::HtmlTagClose, 1);
                self.pop_mode();
                if self.main {
                    self.inside_doctype = false;
                    self.open_pending_element();
                }
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
                ..Default::default()
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
