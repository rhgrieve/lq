//! The token-type switch at the heart of the document parser.

use super::factories::*;
use super::parser::DocumentParser;
use super::tokenizer::TokenType;
use crate::ast::*;
use crate::errors::{Failure, PResult};
use crate::js;

impl<'s> DocumentParser<'s> {
    // node := text | yamlFrontmatter | liquidVariableOutput | liquidTag | htmlElement | htmlComment | htmlDoctype
    pub fn parse_node_inner(&mut self) -> PResult<Node<'s>> {
        let token = self.peek();
        match token.token_type {
            TokenType::Text => Ok(self.advance_as_text()),
            TokenType::YamlFrontmatter => Ok(self.parse_yaml_frontmatter()),
            TokenType::EndOfInput => Err(Failure::parsing(
                "Unexpected end of input in parseNode",
                token.start,
                token.end,
            )),
            TokenType::LiquidVariableOutputOpen => self.parse_liquid_variable_output(),
            TokenType::LiquidTagOpen => self.parse_liquid_tag(),
            TokenType::LiquidTagClose | TokenType::LiquidVariableOutputClose => {
                Ok(self.advance_as_text())
            }
            TokenType::HtmlTagOpen => {
                if self.parse_html {
                    self.parse_html_element()
                } else {
                    Ok(self.advance_as_text())
                }
            }
            TokenType::HtmlCloseTagOpen => {
                if self.parse_html && self.allow_unclosed_html {
                    self.parse_html_dangling_marker_close()
                } else if self.parse_html {
                    Err(self.parse_orphaned_html_close_tag())
                } else {
                    Ok(self.advance_as_text())
                }
            }
            TokenType::HtmlCommentOpen => {
                if self.parse_html {
                    self.parse_html_comment()
                } else {
                    Ok(self.advance_as_text())
                }
            }
            TokenType::HtmlDoctypeOpen => {
                if self.parse_html {
                    self.parse_html_doctype()
                } else {
                    Ok(self.advance_as_text())
                }
            }
            TokenType::HtmlTagClose
            | TokenType::HtmlSelfClose
            | TokenType::HtmlCommentClose
            | TokenType::HtmlEquals
            | TokenType::HtmlQuoteOpen
            | TokenType::HtmlQuoteClose => Ok(self.advance_as_text()),
        }
    }

    // yamlFrontmatter := "---" text "---"
    fn parse_yaml_frontmatter(&mut self) -> Node<'s> {
        let token = self.advance();
        let raw = js::slice(self.source, token.start, token.end);
        let first_newline = memchr::memchr(b'\n', raw.as_bytes());
        let last_dashes = raw.rfind("---");
        let body = match (first_newline, last_dashes) {
            (Some(nl), Some(dashes)) if dashes > nl => &raw[nl + 1..dashes],
            _ => "",
        };
        Node::YamlFrontmatter(Box::new(YamlFrontmatter {
            id: ID0,
            position: Position::new(token.start, token.end),
            body,
        }))
    }

    pub fn advance_as_text(&mut self) -> Node<'s> {
        let token = self.advance();
        make_text_node(js::slice(self.source, token.start, token.end), token.start, token.end)
    }
}
