//! `{{ ... }}`.

use super::factories::*;
use super::parser::DocumentParser;
use super::tokenizer::TokenType;
use crate::ast::*;
use crate::errors::PResult;
use crate::js;
use crate::markup::parser::MarkupParser;
use crate::markup::tokenizer::tokenize_markup;

impl<'s> DocumentParser<'s> {
    // liquidVariableOutput := "{{" liquidVariable "}}"
    pub fn parse_liquid_variable_output(&mut self) -> PResult<Node<'s>> {
        let open_token = self.consume(TokenType::LiquidVariableOutputOpen)?;
        self.accept(TokenType::Text);
        let close_token = self.consume(TokenType::LiquidVariableOutputClose)?;

        let source = self.source;
        let raw_markup = js::slice(source, open_token.end, close_token.start);

        let tokens = tokenize_markup(raw_markup, open_token.end);
        let mut markup_parser = MarkupParser::new(tokens, source, None, None);
        if self.tolerant {
            markup_parser.enable_tolerant();
        }
        let markup = match markup_parser.liquid_variable() {
            Ok(variable) if markup_parser.is_at_end() => Markup::Node(Box::new(variable)),
            _ => Markup::Str(js::trim(raw_markup)),
        };
        Ok(make_liquid_variable_output(&open_token, &close_token, markup))
    }
}
