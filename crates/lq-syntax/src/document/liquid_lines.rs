//! `{% liquid %}` statements.

use super::factories::*;
use super::liquid_blocks::{finalize_branch, is_branch_name, parse_branch_markup_at};
use super::liquid_raw::raw_markup_kind_for_tag;
use super::liquid_tags::UNEXPECTED_TOKENS;
use super::parser::DocumentParser;
use super::tree_builder::{filter_children, ChildFilterMode};
use crate::ast::*;
use crate::errors::{Failure, PResult};
use crate::js;
use crate::markup::parser::MarkupParser;
use crate::markup::tokenizer::tokenize_markup;
use crate::tags::{self, LiquidLineContext, StatementParser, TagDefinition, TagId, ASSERT_NEVER_MESSAGE};
use std::borrow::Cow;

fn empty_delimiters() -> Option<DelimiterWhitespace> {
    Some(DelimiterWhitespace { start: "", end: "" })
}

impl<'s> StatementParser<'s> for DocumentParser<'s> {
    // liquidStatement := tagName markup
    fn parse_liquid_statement(
        &mut self,
        tag_name: &'s str,
        markup_string: &'s str,
        markup_offset: u32,
        ctx: &mut LiquidLineContext<'s>,
    ) -> PResult<Node<'s>> {
        let line = ctx.lines[ctx.index - 1];
        let source = self.source;
        let envelope = envelope_from_line(&line, source);

        if tag_name == "#" {
            return Ok(make_liquid_tag_base_case(&envelope, TagBody::default(), None, true));
        }

        if tag_name.starts_with("end") {
            return Err(Failure::parsing(
                format!("Unexpected end tag '{tag_name}' in {{% liquid %}} block"),
                line.name_offset,
                line.line_end,
            ));
        }

        let Some(def) = tags::tag_for_name(tag_name) else {
            return Ok(make_liquid_tag_base_case(&envelope, TagBody::default(), None, false));
        };

        match def {
            TagDefinition::Tag { id } => {
                let tokens = tokenize_markup(markup_string, markup_offset);
                let mut markup_parser = MarkupParser::new(
                    tokens,
                    source,
                    Some(markup_offset),
                    Some(envelope.markup_end),
                );
                if self.tolerant {
                    markup_parser.enable_tolerant();
                }
                let reason: Cow<'s, str> =
                    match tags::parse_tag(id, tag_name, &mut markup_parser, self) {
                        Ok(markup) => {
                            if markup_parser.is_at_end() {
                                return Ok(make_liquid_tag_named(
                                    &envelope,
                                    markup,
                                    TagBody::default(),
                                ));
                            }
                            Cow::Borrowed(UNEXPECTED_TOKENS)
                        }
                        Err(e) => Cow::Owned(e.into_message()),
                    };
                Ok(make_liquid_tag_base_case(&envelope, TagBody::default(), Some(reason), false))
            }
            TagDefinition::Block { id, branches } => {
                self.parse_line_block_tag(id, branches, &envelope, markup_string, markup_offset, ctx)
            }
            TagDefinition::Raw { .. } => self.parse_line_raw_tag(&envelope, ctx),
            TagDefinition::ObjectPrototype => Err(Failure::error(ASSERT_NEVER_MESSAGE)),
        }
    }
}

impl<'s> DocumentParser<'s> {
    // lineBlockTag := tagName markup LF lineBlockBody | lineBranchedBody "end" tagName
    fn parse_line_block_tag(
        &mut self,
        id: TagId,
        branches: &'static [&'static str],
        envelope: &LiquidTagEnvelope<'s>,
        markup_string: &'s str,
        markup_offset: u32,
        ctx: &mut LiquidLineContext<'s>,
    ) -> PResult<Node<'s>> {
        let tokens = tokenize_markup(markup_string, markup_offset);
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

        let (children, end_position) = if branches.is_empty() {
            self.parse_line_block_body(envelope.tag_name, ctx)?
        } else {
            self.parse_line_branched_body(envelope.tag_name, branches, ctx)?
        };
        let body = TagBody {
            children: Some(children),
            block_end_position: Some(end_position),
            delimiter_whitespace: empty_delimiters(),
        };
        Ok(match parsed {
            Ok(markup) => make_liquid_tag_named(envelope, markup, body),
            Err(reason) => make_liquid_tag_base_case(envelope, body, Some(reason), false),
        })
    }

    // lineRawTag := tagName LF line* "end" tagName
    fn parse_line_raw_tag(
        &mut self,
        envelope: &LiquidTagEnvelope<'s>,
        ctx: &mut LiquidLineContext<'s>,
    ) -> PResult<Node<'s>> {
        let tag_name = envelope.tag_name;
        let source = self.source;
        let is_end_tag = |name: &str| name.strip_prefix("end") == Some(tag_name);

        let in_range = ctx.index > 0 && ctx.index <= ctx.lines.len();
        let position_start_offset =
            if in_range { ctx.lines[ctx.index - 1].line_end } else { envelope.block_start_position.end };
        // One past `lineEnd`: a UTF-16 unit upstream, so the whole char here (a
        // trailing NBSP makes `lineEnd` point at a multi-byte char).
        let body_content_start = if in_range {
            let line_end = ctx.lines[ctx.index - 1].line_end;
            line_end + js::char_width_at(source, line_end as usize).max(1) as u32
        } else {
            envelope.block_start_position.end
        };

        let balanced = tag_name == "comment" || tag_name == "doc";
        let mut end_line_index = None;
        if balanced {
            let mut depth = 0;
            let mut i = ctx.index;
            while i < ctx.lines.len() {
                let name = ctx.lines[i].tag_name;
                if name == "raw" {
                    i += 1;
                    while i < ctx.lines.len() && ctx.lines[i].tag_name != "endraw" {
                        i += 1;
                    }
                    i += 1;
                    continue;
                }
                if name == tag_name {
                    depth += 1;
                } else if is_end_tag(name) {
                    if depth == 0 {
                        end_line_index = Some(i);
                        break;
                    }
                    depth -= 1;
                }
                i += 1;
            }
        } else {
            end_line_index =
                (ctx.index..ctx.lines.len()).find(|&i| is_end_tag(ctx.lines[i].tag_name));
        }

        let Some(end_line_index) = end_line_index else {
            return Err(Failure::parsing(
                format!("Unclosed raw tag '{tag_name}' in {{% liquid %}} block"),
                envelope.block_start_position.start,
                envelope.block_start_position.end,
            ));
        };

        let end_line = ctx.lines[end_line_index];
        let body_end_offset = end_line.name_offset;
        let body_string = js::slice(source, body_content_start, body_end_offset);

        let body_nodes = if body_content_start < body_end_offset {
            filter_children(
                ChildFilterMode::Syntactic,
                vec![make_text_node(body_string, body_content_start, body_end_offset)],
                source,
            )
        } else {
            Vec::new()
        };

        let body = make_raw_markup(
            raw_markup_kind_for_tag(tag_name, body_string),
            body_string,
            body_nodes,
            position_start_offset,
            body_end_offset,
        );

        let end_position = Position::new(
            end_line.name_offset,
            end_line.name_offset + end_line.tag_name.len() as u32,
        );
        ctx.index = end_line_index + 1;

        Ok(make_liquid_raw_tag(
            envelope,
            body,
            end_position,
            DelimiterWhitespace { start: "", end: "" },
        ))
    }

    // lineBlockBody := liquidStatement* "end" tagName
    fn parse_line_block_body(
        &mut self,
        parent_name: &'s str,
        ctx: &mut LiquidLineContext<'s>,
    ) -> PResult<(Vec<Node<'s>>, Position)> {
        let mut children = Vec::new();

        while ctx.index < ctx.lines.len() {
            let line = ctx.lines[ctx.index];
            if line.tag_name.strip_prefix("end") == Some(parent_name) {
                ctx.index += 1;
                let end =
                    Position::new(line.name_offset, line.name_offset + line.tag_name.len() as u32);
                return Ok((children, end));
            }
            ctx.index += 1;
            children.push(self.parse_liquid_statement(
                line.tag_name,
                line.markup,
                line.markup_offset,
                ctx,
            )?);
        }

        if self.allow_unclosed_document_node {
            let end = ctx.lines.last().map_or(0, |l| l.line_end);
            return Ok((children, Position::new(end, end)));
        }

        Err(Failure::parsing(
            format!("Unclosed block tag '{parent_name}' in {{% liquid %}} block"),
            0,
            0,
        ))
    }

    // lineBranchedBody := lineBranch+ "end" tagName
    fn parse_line_branched_body(
        &mut self,
        parent_name: &'s str,
        def_branches: &'static [&'static str],
        ctx: &mut LiquidLineContext<'s>,
    ) -> PResult<(Vec<Node<'s>>, Position)> {
        let source = self.source;
        let mut branches = Vec::new();
        let body_start = if ctx.index > 0 { ctx.lines[ctx.index - 1].line_end } else { 0 };
        let mut current_branch = make_liquid_branch_unnamed(body_start);
        let mut current_children: Vec<Node<'s>> = Vec::new();
        let mode =
            if self.parse_html { ChildFilterMode::StripEdges } else { ChildFilterMode::Syntactic };

        while ctx.index < ctx.lines.len() {
            let line = ctx.lines[ctx.index];

            if line.tag_name.strip_prefix("end") == Some(parent_name) {
                finalize_branch(&mut current_branch, current_children, line.name_offset, source, mode);
                branches.push(Node::LiquidBranch(Box::new(current_branch)));
                ctx.index += 1;
                let end =
                    Position::new(line.name_offset, line.name_offset + line.tag_name.len() as u32);
                return Ok((branches, end));
            }

            if is_branch_name(line.tag_name) && def_branches.contains(&line.tag_name) {
                finalize_branch(
                    &mut current_branch,
                    std::mem::take(&mut current_children),
                    line.name_offset,
                    source,
                    mode,
                );
                let branch_envelope = envelope_from_line(&line, source);
                let branch_markup = parse_branch_markup_at(
                    line.tag_name,
                    &branch_envelope,
                    branch_envelope.markup_offset,
                    source,
                    self.tolerant,
                );
                let next = make_liquid_branch_named(&branch_envelope, branch_markup);
                let finished = std::mem::replace(&mut current_branch, next);
                branches.push(Node::LiquidBranch(Box::new(finished)));
                ctx.index += 1;
                continue;
            }

            ctx.index += 1;
            current_children.push(self.parse_liquid_statement(
                line.tag_name,
                line.markup,
                line.markup_offset,
                ctx,
            )?);
        }

        if self.allow_unclosed_document_node {
            let end = ctx.lines.last().map_or(0, |l| l.line_end);
            finalize_branch(&mut current_branch, current_children, end, source, mode);
            branches.push(Node::LiquidBranch(Box::new(current_branch)));
            return Ok((branches, Position::new(end, end)));
        }

        Err(Failure::parsing(
            format!("Unclosed block tag '{parent_name}' in {{% liquid %}} block"),
            0,
            0,
        ))
    }
}
