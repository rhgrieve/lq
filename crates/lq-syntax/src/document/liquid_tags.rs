//! Tag dispatch.

use super::factories::*;
use super::parser::DocumentParser;
use super::tokenizer::{Token, TokenType};
use crate::ast::*;
use crate::errors::{Failure, PResult};
use crate::markup::parser::MarkupParser;
use crate::markup::tokenizer::tokenize_markup;
use crate::tags::{self, TagDefinition, ASSERT_NEVER_MESSAGE};
use std::borrow::Cow;

pub(crate) const UNEXPECTED_TOKENS: &str = "unexpected tokens after markup";

impl<'s> DocumentParser<'s> {
    // liquidTag := "{%" tagName markup "%}" (block | raw)?
    pub fn parse_liquid_tag(&mut self) -> PResult<Node<'s>> {
        let open_token = self.consume(TokenType::LiquidTagOpen)?;
        self.accept(TokenType::Text);
        let close_token = self.consume(TokenType::LiquidTagClose)?;

        let source = self.source;
        let envelope = envelope_from_tokens(&open_token, &close_token, source);

        if envelope.tag_name.starts_with('#') {
            // The name and markup are contiguous, so the comment is one slice.
            let comment_markup_len =
                envelope.tag_name.len() - 1 + envelope.markup_string.len();
            let comment_markup_offset = envelope.markup_end - comment_markup_len as u32;
            let comment_markup =
                crate::js::slice(source, comment_markup_offset, envelope.markup_end);
            let comment = LiquidTagEnvelope {
                tag_name: "#",
                markup_string: comment_markup,
                markup_offset: comment_markup_offset,
                ..envelope
            };
            return Ok(make_liquid_tag_base_case(&comment, TagBody::default(), None, false));
        }

        if let Some(inner_name) = envelope.tag_name.strip_prefix("end") {
            if tags::tag_for_name(inner_name).is_some() {
                return Err(Failure::parsing(
                    format!(
                        "Attempting to close LiquidTag '{inner_name}' before it was opened without a matching '{inner_name}'"
                    ),
                    open_token.start,
                    close_token.end,
                ));
            }
        }

        let Some(def) = tags::tag_for_name(envelope.tag_name) else {
            return Ok(make_liquid_tag_base_case(&envelope, TagBody::default(), None, false));
        };

        match def {
            TagDefinition::Tag { id } => Ok(self.parse_standalone_tag(id, &envelope, &close_token)),
            TagDefinition::Block { id, branches } => {
                self.parse_block_tag(id, branches, &envelope, &close_token)
            }
            TagDefinition::Raw { parse_liquid_in_body, .. } => {
                self.parse_raw_tag(parse_liquid_in_body, &envelope, &close_token)
            }
            TagDefinition::ObjectPrototype => Err(Failure::error(ASSERT_NEVER_MESSAGE)),
        }
    }

    // standaloneTag := "{%" tagName markup "%}"
    fn parse_standalone_tag(
        &mut self,
        id: tags::TagId,
        envelope: &LiquidTagEnvelope<'s>,
        close_token: &Token,
    ) -> Node<'s> {
        let markup_string_start = close_token.start - envelope.markup_string.len() as u32;
        let markup_string_end = markup_string_start + envelope.markup_string.len() as u32;
        let tokens = tokenize_markup(envelope.markup_string, markup_string_start);
        let mut markup_parser =
            MarkupParser::new(tokens, self.source, Some(markup_string_start), Some(markup_string_end));
        if self.tolerant {
            markup_parser.enable_tolerant();
        }
        let reason: Cow<'s, str> = match tags::parse_tag(id, envelope.tag_name, &mut markup_parser, self) {
            Ok(markup) => {
                if markup_parser.is_at_end() {
                    return make_liquid_tag_named(envelope, markup, TagBody::default());
                }
                Cow::Borrowed(UNEXPECTED_TOKENS)
            }
            Err(e) => Cow::Owned(e.into_message()),
        };
        make_liquid_tag_base_case(envelope, TagBody::default(), Some(reason), false)
    }
}
