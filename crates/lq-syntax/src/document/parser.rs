//! `DocumentParser` and `ParserBase`: token cursor, entry point, tolerant recovery.

use super::factories::*;
use super::tokenizer::{tokenize_from, Token, TokenType, TokenizeOptions};
use super::tree_builder::{filter_children, ChildFilterMode};
use crate::ast::*;
use crate::errors::{Failure, PResult};
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
        }
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
            TokenizeOptions { skip_frontmatter: true, inside_quoted_attribute, inside_html_tag },
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
                while !self.is_at_end() && !is_resync_token(self.peek().token_type) {
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
}
