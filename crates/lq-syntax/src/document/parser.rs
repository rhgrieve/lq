//! `DocumentParser` and `ParserBase`: token cursor, entry point, tolerant recovery.

use super::factories::*;
use super::raw_body::{RawBodyEnd, RawBodyEndCache};
use super::tokenizer::{tokenize, tokenize_from, Token, TokenType, TokenizeOptions};
use super::tree_builder::{filter_children, ChildFilterMode};
use crate::ast::*;
use crate::errors::{Failure, PResult, UnclosedNode};
use crate::tags::{self, TagDefinition};
use crate::ParseTarget;
use std::borrow::Cow;

pub(crate) struct DocumentParser<'s> {
    pub tokens: Vec<Token>,
    pub source: &'s str,
    pub p: usize,
    pub parse_html: bool,
    pub allow_unclosed_html: bool,
    pub allow_unclosed_document_node: bool,
    pub in_attribute_context: bool,
    pub in_attribute_value_context: bool,
    pub tolerant: bool,
    pub target: ParseTarget,
    /// `ParseTarget::Main`, cached.
    pub main: bool,
    /// `_openHtmlComment`: the `<!--` of the comment the parser is inside, or
    /// `None` outside a comment. Only `Main` sets it.
    pub open_html_comment: Option<Position>,
    /// `rawBodyEnds`: the tokenizer's raw body ends by close token index, until a
    /// retokenize renumbers the tokens.
    pub raw_body_ends: Vec<(usize, RawBodyEnd)>,
    pub raw_body_end_cache: RawBodyEndCache,
}

/// Token types that open a construct; panic-mode recovery resyncs onto these.
fn is_resync_token(token_type: TokenType) -> bool {
    matches!(
        token_type,
        TokenType::LiquidTagOpen
            | TokenType::LiquidVariableOutputOpen
            | TokenType::HtmlTagOpen
            | TokenType::HtmlCloseTagOpen
            | TokenType::HtmlCommentOpen
            | TokenType::HtmlDoctypeOpen
            | TokenType::EndOfInput
    )
}

impl<'s> DocumentParser<'s> {
    pub fn new(
        tokens: Vec<Token>,
        source: &'s str,
        parse_html: bool,
        allow_unclosed_document_node: bool,
        tolerant: bool,
        target: ParseTarget,
    ) -> Self {
        DocumentParser {
            tokens,
            source,
            p: 0,
            parse_html,
            allow_unclosed_html: allow_unclosed_document_node,
            allow_unclosed_document_node,
            in_attribute_context: false,
            in_attribute_value_context: false,
            tolerant,
            target,
            main: target.is_main(),
            open_html_comment: None,
            raw_body_ends: Vec::new(),
            raw_body_end_cache: RawBodyEndCache::default(),
        }
    }

    /// `env.tagForName(name)`. On `main` the builtins are a `Map`, so a name like
    /// `constructor` is no tag at all (a73b174f).
    #[inline]
    pub fn tag_for_name(&self, name: &str) -> Option<TagDefinition> {
        match tags::tag_for_name(name) {
            Some(TagDefinition::ObjectPrototype) if self.main => None,
            def => def,
        }
    }

    /// The `unclosed` of an error, which only `main` reports (33b1f3aa).
    #[inline]
    pub fn unclosed(&self, node_type: &'static str, name: &str, block_start_position: Position) -> Option<UnclosedNode> {
        self.main.then(|| UnclosedNode { node_type, name: name.to_string(), block_start_position })
    }

    /// `rawBodyEndFor(closeToken)`: where the tokenizer ended the body of the raw
    /// tag whose close token was just consumed.
    pub fn raw_body_end_for(&self, close_token: &Token) -> Option<RawBodyEnd> {
        let index = self.p.checked_sub(1)?;
        if self.tokens.get(index) != Some(close_token) {
            return None;
        }
        let i = self.raw_body_ends.binary_search_by_key(&index, |&(i, _)| i).ok()?;
        Some(self.raw_body_ends[i].1)
    }

    /// `tokenizeSlice`: tokens for `source[start..end]`, with offsets in the whole
    /// source, under `retokenizeRawText`'s options (`rawTags` but not
    /// `builtinRawTags`). Drops the slice's EndOfInput unless `with_end` is set.
    fn tokenize_slice(&self, start: u32, end: u32, raw_text: bool, with_end: bool) -> Vec<Token> {
        let mut tokens = tokenize(
            &self.source[start as usize..end as usize],
            TokenizeOptions {
                skip_frontmatter: true,
                target: ParseTarget::Main,
                raw_tags: true,
                raw_text,
                ..TokenizeOptions::default()
            },
        );
        if !with_end {
            tokens.pop();
        }
        for t in &mut tokens {
            t.start += start;
            t.end += start;
        }
        tokens
    }

    /// `retokenizeRawText`: re-tokenize the `<script>`/`<style>` body from
    /// `body_start` to `body_end` as raw text, with the close tag
    /// `body_end..close_end` and the rest of the source as a normal document.
    /// After the close tag the existing tokens are kept when the old stream
    /// already resumed a normal document there. Leaves the cursor at the first
    /// body token.
    pub fn retokenize_raw_text(&mut self, body_start: u32, body_end: u32, close_end: u32) {
        self.raw_body_ends.clear();
        let prefix_len = self.tokens.partition_point(|t| t.end <= body_start);
        let body = self.tokenize_slice(body_start, body_end, true, false);
        let close_index = self
            .tokens
            .iter()
            .position(|t| t.token_type == TokenType::HtmlTagClose && t.end == close_end);
        let resumes = close_index
            .is_some_and(|i| self.tokens.get(i + 1).is_some_and(|t| t.start == close_end));
        let mut suffix = match close_index {
            Some(i) if resumes => {
                let mut s = self.tokenize_slice(body_end, close_end, false, false);
                s.extend_from_slice(&self.tokens[i + 1..]);
                s
            }
            _ => self.tokenize_slice(body_end, self.source.len() as u32, false, true),
        };
        self.tokens.truncate(prefix_len);
        self.tokens.extend(body);
        self.tokens.append(&mut suffix);
        self.p = prefix_len;
    }

    // ---- ParserBase ----

    #[inline]
    pub fn consume(&mut self, token_type: TokenType) -> PResult<Token> {
        let token = self.tokens[self.p];
        if token.token_type != token_type {
            return Err(Failure::parsing(
                format!("Expected {} but got {}", token_type.as_str(), token.token_type.as_str()),
                token.start,
                token.end,
            ));
        }
        self.p += 1;
        Ok(token)
    }

    #[inline]
    pub fn accept(&mut self, token_type: TokenType) -> Option<Token> {
        let token = self.tokens[self.p];
        if token.token_type == token_type {
            self.p += 1;
            return Some(token);
        }
        None
    }

    #[inline]
    pub fn check(&self, token_type: TokenType) -> bool {
        self.check_ahead(token_type, 0)
    }

    #[inline]
    pub fn check_ahead(&self, token_type: TokenType, ahead: usize) -> bool {
        self.tokens.get(self.p + ahead).is_some_and(|t| t.token_type == token_type)
    }

    #[inline]
    pub fn peek(&self) -> Token {
        self.tokens[self.p]
    }

    #[inline]
    pub fn advance(&mut self) -> Token {
        let token = self.tokens[self.p];
        self.p += 1;
        token
    }

    #[inline]
    pub fn is_at_end(&self) -> bool {
        self.tokens[self.p].token_type == TokenType::EndOfInput
    }

    /// Tokens are sorted and never overlap, so upstream's linear scan for the
    /// first token at or after an offset is a binary search here.
    #[inline]
    fn first_token_at_or_after(&self, source_offset: u32) -> usize {
        self.tokens.partition_point(|t| t.start < source_offset)
    }

    pub fn seek_to_source_offset(&mut self, source_offset: u32) {
        self.p = self.first_token_at_or_after(source_offset);
    }

    fn open_quote_char_before(&self, prefix: &[Token]) -> Option<char> {
        let last = prefix.iter().rev().find(|t| {
            matches!(t.token_type, TokenType::HtmlQuoteOpen | TokenType::HtmlQuoteClose)
        })?;
        if last.token_type == TokenType::HtmlQuoteClose {
            return None;
        }
        self.source.get(last.start as usize..).and_then(|s| s.chars().next())
    }

    fn inside_open_html_tag(prefix: &[Token]) -> bool {
        prefix
            .iter()
            .rev()
            .find_map(|t| match t.token_type {
                TokenType::HtmlTagOpen
                | TokenType::HtmlCloseTagOpen
                | TokenType::HtmlDoctypeOpen => Some(true),
                TokenType::HtmlTagClose | TokenType::HtmlSelfClose => Some(false),
                _ => None,
            })
            .unwrap_or(false)
    }

    /// See `resliceTokensFrom` in document/base.ts. Only a token straddling the
    /// boundary forces a re-tokenize; otherwise this just moves the cursor.
    pub fn reslice_tokens_from(&mut self, source_offset: u32) {
        let first = self.first_token_at_or_after(source_offset);
        let straddles = first > 0 && self.tokens[first - 1].end > source_offset;
        if !straddles {
            self.p = first;
            return;
        }
        let prefix_len = first - 1;
        let prefix = &self.tokens[..prefix_len];
        let inside_quoted_attribute = self.open_quote_char_before(prefix);
        let inside_html_tag =
            inside_quoted_attribute.is_none() && Self::inside_open_html_tag(prefix);
        self.tokens.truncate(prefix_len);
        tokenize_from(
            self.source,
            source_offset as usize,
            TokenizeOptions {
                skip_frontmatter: true,
                inside_quoted_attribute,
                inside_html_tag,
                ..TokenizeOptions::default()
            },
            &mut self.tokens,
        );
        self.p = prefix_len;
    }

    // ---- DocumentParser ----

    // document := yamlFrontmatter? node*
    pub fn parse_document(&mut self) -> PResult<Node<'s>> {
        let mut children = Vec::new();
        if self.check(TokenType::YamlFrontmatter) {
            children.push(self.parse_node_inner()?);
        }
        while !self.is_at_end() {
            children.push(self.parse_node()?);
        }
        // A comment still open at the end of the input ends there, as in a browser.
        self.open_html_comment = None;
        Ok(make_document_node(
            filter_children(ChildFilterMode::Syntactic, children, self.source),
            self.source,
        ))
    }

    /// `parseNode`, including `TolerantDocumentParser`'s override: on a parsing
    /// failure emit a `LiquidErrorNode` over the skipped region and resync.
    pub fn parse_node(&mut self) -> PResult<Node<'s>> {
        if !self.tolerant {
            return self.parse_node_inner();
        }
        let start_tok = self.peek();
        let start_pos = self.p;
        match self.parse_node_inner() {
            Ok(node) => Ok(node),
            Err(Failure::Parsing { message, .. }) => {
                let found = self.peek().token_type.as_str();
                if self.p <= start_pos {
                    self.advance();
                }
                while !self.is_at_end() && !self.is_resync_point() {
                    self.advance();
                }
                let end = self.peek().start;
                Ok(Node::LiquidErrorNode(Box::new(LiquidErrorNode {
                    id: ID0,
                    position: Position::new(start_tok.start, end),
                    message: Cow::Owned(message),
                    found: Some(found),
                })))
            }
            Err(other) => Err(other),
        }
    }

    fn is_resync_point(&self) -> bool {
        let token_type = self.peek().token_type;
        // Skipping the open comment's `-->` would make the rest of the document
        // comment text.
        is_resync_token(token_type)
            || (token_type == TokenType::HtmlCommentClose && self.open_html_comment.is_some())
    }
}
