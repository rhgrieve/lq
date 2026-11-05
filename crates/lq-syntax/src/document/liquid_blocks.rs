//! Block tag bodies and branches.

use super::factories::*;
use super::liquid_tags::UNEXPECTED_TOKENS;
use super::parser::DocumentParser;
use super::tokenizer::{Token, TokenType};
use super::tree_builder::{filter_children, ChildFilterMode};
use crate::ast::*;
use crate::errors::{Failure, PResult};
use crate::js;
use crate::markup::parser::MarkupParser;
use crate::markup::tokenizer::tokenize_markup;
use crate::tags::{self, TagId};
use std::borrow::Cow;

pub(crate) fn is_branch_name(value: &str) -> bool {
    matches!(value, "elsif" | "else" | "when")
}

pub(crate) struct BlockEnd {
    pub position: Position,
    pub whitespace: DelimiterWhitespace,
}

pub(crate) fn finalize_branch<'s>(
    branch: &mut LiquidBranch<'s>,
    children: Vec<Node<'s>>,
    end_pos: u32,
    source: &'s str,
    mode: ChildFilterMode,
) {
    // `allMerged`'s last element ends where the last child ends.
    let last_child_end = children.last().map(|c| c.position().end);
    branch.children = filter_children(mode, children, source);
    branch.block_end_position = Position::new(end_pos, end_pos);
    branch.position.end = end_pos;
    if let Some(end) = last_child_end {
        if end > end_pos {
            branch.position.end = end;
        }
    }
}

impl<'s> DocumentParser<'s> {
    // blockTag := "{%" tagName markup "%}" branchedBody | blockBody "{%" "end" tagName "%}"
    pub fn parse_block_tag(
        &mut self,
        id: TagId,
        branches: &'static [&'static str],
        envelope: &LiquidTagEnvelope<'s>,
        close_token: &Token,
    ) -> PResult<Node<'s>> {
        let markup_string_start = close_token.start - envelope.markup_string.len() as u32;
        let tokens = tokenize_markup(envelope.markup_string, markup_string_start);
        let mut markup_parser = MarkupParser::new(tokens, self.source, None, None);
        if self.tolerant {
            markup_parser.enable_tolerant();
        }
        let parsed: Result<Markup<'s>, Cow<'s, str>> =
            match tags::parse_tag(id, envelope.tag_name, &mut markup_parser, self) {
                Ok(markup) if markup_parser.is_at_end() => Ok(markup),
                Ok(_) => Err(Cow::Borrowed(UNEXPECTED_TOKENS)),
                Err(e) => Err(Cow::Owned(e.into_message())),
            };

        let saved_allow_unclosed = self.allow_unclosed_html;
        self.allow_unclosed_html = matches!(envelope.tag_name, "if" | "unless" | "case");

        let result = if branches.is_empty() {
            self.parse_block_body(envelope)
        } else {
            self.parse_branched_body(envelope, branches)
        };
        // The `finally`: this must also hold when the body threw.
        self.allow_unclosed_html = saved_allow_unclosed;
        let (children, end) = result?;

        let body = TagBody {
            children: Some(children),
            block_end_position: Some(end.position),
            delimiter_whitespace: Some(end.whitespace),
        };
        Ok(match parsed {
            Ok(markup) => make_liquid_tag_named(envelope, markup, body),
            Err(reason) => make_liquid_tag_base_case(envelope, body, Some(reason), false),
        })
    }

    fn closing_error(&self, envelope: &LiquidTagEnvelope<'s>, inner_name: &str) -> Failure {
        Failure::parsing(
            format!(
                "Attempting to close LiquidTag '{inner_name}' before LiquidTag '{}' was closed",
                envelope.tag_name
            ),
            envelope.block_start_position.start,
            envelope.block_start_position.end,
        )
    }

    /// Reports an iteration that consumed nothing. `Ok(true)` means keep looping,
    /// `Ok(false)` means break.
    fn handle_stall(
        &mut self,
        parent_name: &str,
        children: &mut Vec<Node<'s>>,
    ) -> PResult<bool> {
        if !self.allow_unclosed_document_node {
            let tok = self.peek();
            let name = self.peek_tag_name();
            return Err(Failure::parsing(
                format!(
                    "Unexpected Liquid tag '{}' inside LiquidTag '{parent_name}'",
                    name.unwrap_or(tok.token_type.as_str())
                ),
                tok.start,
                tok.end,
            ));
        }
        if self.check(TokenType::LiquidTagOpen) {
            children.push(self.parse_liquid_tag()?);
            return Ok(true);
        }
        Ok(false)
    }

    fn parse_body_item(&mut self, children: &mut Vec<Node<'s>>) -> PResult<()> {
        if self.in_attribute_context && !self.in_attribute_value_context {
            let attrs = self.parse_branch_attributes()?;
            children.extend(attrs);
        } else {
            children.push(self.parse_node()?);
        }
        Ok(())
    }

    // blockBody := node* "{%" "end" tagName "%}"
    fn parse_block_body(
        &mut self,
        envelope: &LiquidTagEnvelope<'s>,
    ) -> PResult<(Vec<Node<'s>>, BlockEnd)> {
        let parent_name = envelope.tag_name;
        let mut children = Vec::new();

        while !self.is_at_end() {
            if self.check(TokenType::LiquidTagOpen) {
                let tag_name = self.peek_tag_name();
                if let Some(name) = tag_name {
                    if let Some(inner_name) = name.strip_prefix("end") {
                        if inner_name == parent_name {
                            let end = self.consume_end_tag()?;
                            let merged = filter_children(
                                ChildFilterMode::Syntactic,
                                children,
                                self.source,
                            );
                            return Ok((merged, end));
                        }
                        if tags::tag_for_name(inner_name).is_some() {
                            return Err(self.closing_error(envelope, inner_name));
                        }
                    }
                    if is_branch_name(name) {
                        return Err(Failure::parsing(
                            format!(
                                "Attempting to open LiquidBranch '{name}' before LiquidTag '{parent_name}' was closed"
                            ),
                            envelope.block_start_position.start,
                            envelope.block_start_position.end,
                        ));
                    }
                }
            }
            let progress_start = self.p;
            self.parse_body_item(&mut children)?;
            if self.p == progress_start {
                if self.handle_stall(parent_name, &mut children)? {
                    continue;
                }
                break;
            }
        }

        let merged = filter_children(ChildFilterMode::Syntactic, children, self.source);
        let end_pos = self.peek().start;
        if self.allow_unclosed_document_node {
            return Ok((
                merged,
                BlockEnd {
                    position: Position::new(end_pos, end_pos),
                    whitespace: DelimiterWhitespace { start: "", end: "" },
                },
            ));
        }
        Err(Failure::parsing(
            format!("Attempting to end parsing before LiquidTag '{parent_name}' was closed"),
            envelope.block_start_position.start,
            envelope.block_start_position.end,
        ))
    }

    // branchedBody := branch+ "{%" "end" tagName "%}"
    fn parse_branched_body(
        &mut self,
        envelope: &LiquidTagEnvelope<'s>,
        def_branches: &'static [&'static str],
    ) -> PResult<(Vec<Node<'s>>, BlockEnd)> {
        let parent_name = envelope.tag_name;
        let source = self.source;
        let body_start = envelope.block_start_position.end;
        let mut branches: Vec<Node<'s>> = Vec::new();
        let mut current_branch = make_liquid_branch_unnamed(body_start);
        let mut current_children: Vec<Node<'s>> = Vec::new();
        let mode = if self.in_attribute_value_context {
            ChildFilterMode::Preserve
        } else {
            ChildFilterMode::Syntactic
        };

        while !self.is_at_end() {
            if self.check(TokenType::LiquidTagOpen) {
                let tag_name = self.peek_tag_name().unwrap_or("");
                let inner_name = tag_name.strip_prefix("end");

                if inner_name == Some(parent_name) {
                    finalize_branch(
                        &mut current_branch,
                        current_children,
                        self.peek().start,
                        source,
                        mode,
                    );
                    branches.push(Node::LiquidBranch(Box::new(current_branch)));
                    let end = self.consume_end_tag()?;
                    return Ok((branches, end));
                }

                if is_branch_name(tag_name) && def_branches.contains(&tag_name) {
                    finalize_branch(
                        &mut current_branch,
                        std::mem::take(&mut current_children),
                        self.peek().start,
                        source,
                        mode,
                    );
                    let open_token = self.consume(TokenType::LiquidTagOpen)?;
                    // The finished branch is pushed before the consumes can
                    // throw in upstream; a throw discards `branches` anyway.
                    self.accept(TokenType::Text);
                    let close_token = self.consume(TokenType::LiquidTagClose)?;
                    let branch_envelope = envelope_from_tokens(&open_token, &close_token, source);
                    let branch_markup = self.parse_branch_markup(tag_name, &branch_envelope);
                    let next = make_liquid_branch_named(&branch_envelope, branch_markup);
                    let finished = std::mem::replace(&mut current_branch, next);
                    branches.push(Node::LiquidBranch(Box::new(finished)));
                    continue;
                }

                if let Some(inner_name) = inner_name {
                    if tags::tag_for_name(inner_name).is_some() {
                        return Err(self.closing_error(envelope, inner_name));
                    }
                }
            }
            let progress_start = self.p;
            self.parse_body_item(&mut current_children)?;
            if self.p == progress_start {
                if self.handle_stall(parent_name, &mut current_children)? {
                    continue;
                }
                break;
            }
        }

        let end_pos = self.peek().start;
        finalize_branch(&mut current_branch, current_children, end_pos, source, mode);
        branches.push(Node::LiquidBranch(Box::new(current_branch)));
        if self.allow_unclosed_document_node {
            return Ok((
                branches,
                BlockEnd {
                    position: Position::new(end_pos, end_pos),
                    whitespace: DelimiterWhitespace { start: "", end: "" },
                },
            ));
        }
        Err(Failure::parsing(
            format!("Attempting to end parsing before LiquidTag '{parent_name}' was closed"),
            envelope.block_start_position.start,
            envelope.block_start_position.end,
        ))
    }

    // branchMarkup := elsifMarkup | whenMarkup | ""
    fn parse_branch_markup(
        &mut self,
        branch_name: &str,
        envelope: &LiquidTagEnvelope<'s>,
    ) -> Markup<'s> {
        let close_token_width = if envelope.whitespace_end == "-" { 3 } else { 2 };
        let close_token_start = envelope.block_start_position.end - close_token_width;
        let markup_string_start = close_token_start - envelope.markup_string.len() as u32;
        parse_branch_markup_at(
            branch_name,
            envelope,
            markup_string_start,
            self.source,
            self.tolerant,
        )
    }

    pub fn peek_tag_name(&self) -> Option<&'s str> {
        if !self.check(TokenType::LiquidTagOpen) {
            return None;
        }
        let inner = if self.check_ahead(TokenType::Text, 1) {
            let t = self.tokens[self.p + 1];
            js::slice(self.source, t.start, t.end)
        } else {
            ""
        };
        let trimmed = js::trim_start(inner);
        Some(match js::find_ws(trimmed) {
            None => trimmed,
            Some(i) => &trimmed[..i],
        })
    }

    pub fn is_block_terminator(&self) -> bool {
        match self.peek_tag_name() {
            None | Some("") => false,
            Some(name) => name.starts_with("end") || is_branch_name(name),
        }
    }

    fn consume_end_tag(&mut self) -> PResult<BlockEnd> {
        let open_token = self.consume(TokenType::LiquidTagOpen)?;
        self.accept(TokenType::Text);
        let close_token = self.consume(TokenType::LiquidTagClose)?;
        Ok(BlockEnd {
            position: Position::new(open_token.start, close_token.end),
            whitespace: DelimiterWhitespace {
                start: marker(&open_token),
                end: marker(&close_token),
            },
        })
    }
}

/// Shared by the `{% tag %}` and `{% liquid %}` branch paths: parse `elsif`/`when`
/// markup, falling back to the trimmed string on any failure.
pub(crate) fn parse_branch_markup_at<'s>(
    branch_name: &str,
    envelope: &LiquidTagEnvelope<'s>,
    markup_string_start: u32,
    source: &'s str,
    tolerant: bool,
) -> Markup<'s> {
    if branch_name == "else" {
        return Markup::Str("");
    }
    let tokens = tokenize_markup(envelope.markup_string, markup_string_start);
    let mut markup_parser = MarkupParser::new(tokens, source, None, None);
    if tolerant {
        markup_parser.enable_tolerant();
    }
    let parsed = if branch_name == "elsif" {
        tags::elsif_branch_parse(&mut markup_parser).map(|n| Markup::Node(Box::new(n)))
    } else {
        tags::when_branch_parse(&mut markup_parser).map(Markup::List)
    };
    match parsed {
        Ok(markup) if markup_parser.is_at_end() => markup,
        _ => Markup::Str(js::trim(envelope.markup_string)),
    }
}
