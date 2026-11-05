//! `{% doc %}` bodies.

use super::tokenizer::*;
use crate::ast::*;
use crate::document::factories::{make_text_node, ID0};
use crate::js;
use std::borrow::Cow;

const DESCRIPTION: &str = "description";

pub(crate) fn parse_liquid_doc<'s>(
    body: &'s str,
    start_offset: u32,
    source: &'s str,
) -> Vec<Node<'s>> {
    LiquidDocParser { tokens: tokenize_liquid_doc(body, start_offset), source, p: 0 }.parse()
}

struct LiquidDocParser<'s> {
    tokens: Vec<LiquidDocToken<'s>>,
    source: &'s str,
    p: usize,
}

#[derive(Default)]
struct CollectOptions {
    stop_at_paragraph_break: bool,
    trim_trailing_newlines: bool,
}

struct Collected {
    value: String,
    start_pos: u32,
    end_pos: u32,
}

fn is_blank_text(token: &LiquidDocToken<'_>) -> bool {
    token.token_type == LiquidDocTokenType::TextLine && js::trim(token.value).is_empty()
}

fn description_node<'s>(
    content: Node<'s>,
    is_implicit: bool,
    is_inline: bool,
    start: u32,
    end: u32,
) -> Node<'s> {
    Node::LiquidDocDescriptionNode(Box::new(LiquidDocDescriptionNode {
        id: ID0,
        position: Position::new(start, end),
        name: "description",
        content,
        is_implicit,
        is_inline,
    }))
}

fn example_node<'s>(content: Node<'s>, is_inline: bool, start: u32, end: u32) -> Node<'s> {
    Node::LiquidDocExampleNode(Box::new(LiquidDocExampleNode {
        id: ID0,
        position: Position::new(start, end),
        name: "example",
        content,
        is_inline,
    }))
}

fn param_node<'s>(
    param_name: Node<'s>,
    param_type: Option<Node<'s>>,
    param_description: Option<Node<'s>>,
    required: bool,
    start: u32,
    end: u32,
) -> Node<'s> {
    Node::LiquidDocParamNode(Box::new(LiquidDocParamNode {
        id: ID0,
        position: Position::new(start, end),
        name: "param",
        param_name,
        param_type,
        param_description,
        required,
    }))
}

impl<'s> LiquidDocParser<'s> {
    fn parse(&mut self) -> Vec<Node<'s>> {
        use LiquidDocTokenType as T;
        let mut nodes = Vec::new();

        self.skip_blank_lines();

        if self.check(T::TextLine) {
            nodes.push(self.parse_implicit_description());
        }

        self.skip_blank_lines();

        while !self.is_at_end() {
            if self.check(T::Newline) {
                while self.check(T::Newline) {
                    self.advance();
                }
                continue;
            }
            if self.check(T::Annotation) {
                nodes.push(self.parse_annotation());
                continue;
            }
            if is_blank_text(&self.peek()) {
                self.advance();
                continue;
            }
            if self.check(T::TextLine) {
                let c = self.collect_content_lines(CollectOptions {
                    trim_trailing_newlines: true,
                    ..Default::default()
                });
                if !js::trim(&c.value).is_empty() {
                    nodes.push(make_text_node(c.value, c.start_pos, c.end_pos));
                }
                continue;
            }
            let tok = self.advance();
            nodes.push(make_text_node(tok.value, tok.start, tok.end));
        }
        nodes
    }

    fn parse_implicit_description(&mut self) -> Node<'s> {
        let c = self.collect_content_lines(CollectOptions::default());
        let content = make_text_node(c.value, c.start_pos, c.end_pos);
        description_node(content, true, true, c.start_pos, c.end_pos)
    }

    fn parse_annotation(&mut self) -> Node<'s> {
        let annotation_token = self.advance();
        let name = annotation_token.value;

        if name != DESCRIPTION && name.starts_with(DESCRIPTION) {
            return self.parse_glued_description(annotation_token);
        }

        match name {
            "param" => self.parse_param(annotation_token),
            "description" => self.parse_description(annotation_token),
            "example" => self.parse_example(annotation_token),
            "prompt" => self.parse_prompt(annotation_token),
            _ => self.parse_unsupported_annotation(annotation_token),
        }
    }

    // param := "@param" ("{" type "}")? ("[" name "]" | name) ("-" description)?
    fn parse_param(&mut self, annotation_token: LiquidDocToken<'s>) -> Node<'s> {
        let start = annotation_token.start;
        let empty_name = || make_text_node("", annotation_token.end, annotation_token.end);

        let Some(inline_token) = self.accept(LiquidDocTokenType::TextLine) else {
            return param_node(empty_name(), None, None, true, start, annotation_token.end);
        };

        let param_tokens = tokenize_param_content(inline_token.value, inline_token.start);
        let mut q = 0usize;
        let mut accept = |ty: ParamTokenType| {
            if param_tokens[q].token_type == ty {
                q += 1;
                Some(param_tokens[q - 1])
            } else {
                None
            }
        };

        let param_type = accept(ParamTokenType::Type)
            .map(|t| make_text_node(t.value, t.start + 1, t.end - 1));

        let (param_name, required) = if let Some(t) = accept(ParamTokenType::OptionalName) {
            (make_text_node(t.value, t.start + 1, t.end - 1), false)
        } else if let Some(t) = accept(ParamTokenType::Word) {
            (make_text_node(t.value, t.start, t.end), true)
        } else {
            (empty_name(), true)
        };

        let dash_token = accept(ParamTokenType::Dash);

        let mut inline_desc_range: Option<(u32, u32)> = None;
        while param_tokens[q].token_type != ParamTokenType::EndOfInput {
            let t = param_tokens[q];
            q += 1;
            inline_desc_range = Some((inline_desc_range.map_or(t.start, |r| r.0), t.end));
        }
        let inline_desc =
            inline_desc_range.map_or("", |(a, b)| js::slice(self.source, a, b));

        let cont = self.collect_content_lines(CollectOptions {
            stop_at_paragraph_break: true,
            trim_trailing_newlines: true,
        });
        let has_continuation = !js::trim(&cont.value).is_empty();

        let mut param_description = None;
        if !inline_desc.is_empty() || has_continuation {
            let full_desc: Cow<'s, str> = if has_continuation {
                Cow::Owned(format!("{inline_desc}{}", cont.value))
            } else {
                Cow::Borrowed(inline_desc)
            };
            let desc_start = match (inline_desc_range, dash_token) {
                (Some((a, _)), _) => a,
                (None, Some(dash)) => dash.end + 1,
                (None, None) => annotation_token.end,
            };
            let desc_end = if has_continuation {
                cont.end_pos
            } else {
                inline_desc_range.map_or(0, |r| r.1)
            };
            param_description = Some(make_text_node(full_desc, desc_start, desc_end));
        }

        let end = param_description.as_ref().map_or(inline_token.end, |d| d.position().end);
        param_node(param_name, param_type, param_description, required, start, end)
    }

    /// The inline-or-following-lines content shape of `@description`/`@example`.
    /// Returns (content, end, is_inline).
    fn parse_block_content(&mut self, annotation_token: &LiquidDocToken<'s>) -> (Node<'s>, u32, bool) {
        if let Some(inline_token) = self.accept(LiquidDocTokenType::TextLine) {
            let rest = self.collect_content_lines(CollectOptions::default());
            let has_cont = !js::trim(&rest.value).is_empty();
            let (full, end): (Cow<'s, str>, u32) = if has_cont {
                (Cow::Owned(format!("{}{}", inline_token.value, rest.value)), rest.end_pos)
            } else {
                (Cow::Borrowed(inline_token.value), inline_token.end)
            };
            return (make_text_node(full, inline_token.start, end), end, true);
        }
        self.skip_blank_lines();
        let c = self.collect_content_lines(CollectOptions::default());
        let has_value = !c.value.is_empty();
        let content_start = if has_value { c.start_pos } else { annotation_token.end };
        let end = if has_value { c.end_pos } else { content_start };
        (make_text_node(c.value, content_start, end), end, false)
    }

    fn parse_description(&mut self, annotation_token: LiquidDocToken<'s>) -> Node<'s> {
        let (content, end, is_inline) = self.parse_block_content(&annotation_token);
        description_node(content, false, is_inline, annotation_token.start, end)
    }

    // `@descriptionText`: the glued suffix and same-line text are the description,
    // read straight from source so their spacing survives.
    fn parse_glued_description(&mut self, annotation_token: LiquidDocToken<'s>) -> Node<'s> {
        let start = annotation_token.start;
        let content_start = start + 1 + DESCRIPTION.len() as u32;

        let inline_token = self.accept(LiquidDocTokenType::TextLine);
        let rest = self.collect_content_lines(CollectOptions::default());
        let has_rest = !js::trim(&rest.value).is_empty();

        let end = if has_rest {
            rest.end_pos
        } else if let Some(t) = inline_token {
            t.end
        } else {
            annotation_token.end
        };

        let content = make_text_node(js::slice(self.source, content_start, end), content_start, end);
        description_node(content, false, true, start, end)
    }

    fn parse_example(&mut self, annotation_token: LiquidDocToken<'s>) -> Node<'s> {
        let (content, end, is_inline) = self.parse_block_content(&annotation_token);
        example_node(content, is_inline, annotation_token.start, end)
    }

    fn parse_prompt(&mut self, annotation_token: LiquidDocToken<'s>) -> Node<'s> {
        let c = self.collect_content_lines(CollectOptions::default());
        let has_value = !c.value.is_empty();
        let content_start = if has_value { c.start_pos } else { annotation_token.end };
        let end = if has_value { c.end_pos } else { content_start };
        Node::LiquidDocPromptNode(Box::new(LiquidDocPromptNode {
            id: ID0,
            position: Position::new(annotation_token.start, end),
            name: "prompt",
            content: make_text_node(c.value, content_start, end),
        }))
    }

    fn parse_unsupported_annotation(&mut self, annotation_token: LiquidDocToken<'s>) -> Node<'s> {
        let start = annotation_token.start;
        let end = match self.accept(LiquidDocTokenType::TextLine) {
            Some(t) => t.end,
            None => annotation_token.end,
        };
        make_text_node(js::slice(self.source, start, end), start, end)
    }

    #[inline]
    fn peek(&self) -> LiquidDocToken<'s> {
        self.tokens[self.p]
    }

    fn accept(&mut self, ty: LiquidDocTokenType) -> Option<LiquidDocToken<'s>> {
        if self.peek().token_type != ty {
            return None;
        }
        Some(self.advance())
    }

    #[inline]
    fn check(&self, ty: LiquidDocTokenType) -> bool {
        self.tokens[self.p].token_type == ty
    }

    #[inline]
    fn is_at_end(&self) -> bool {
        self.check(LiquidDocTokenType::EndOfInput)
    }

    fn advance(&mut self) -> LiquidDocToken<'s> {
        self.p += 1;
        self.tokens[self.p - 1]
    }

    fn skip_blank_lines(&mut self) {
        while !self.is_at_end() {
            if self.check(LiquidDocTokenType::Newline) || is_blank_text(&self.peek()) {
                self.advance();
                continue;
            }
            break;
        }
    }

    fn collect_content_lines(&mut self, opts: CollectOptions) -> Collected {
        use LiquidDocTokenType as T;
        let mut value = String::new();
        let mut start_pos = self.tokens[self.p].start;
        let mut end_pos = start_pos;

        while !self.is_at_end() {
            let current = self.peek();

            if self.check(T::Annotation) {
                break;
            }

            if self.check(T::Newline) {
                let mut look_ahead = self.p + 1;
                while look_ahead < self.tokens.len() {
                    let la = &self.tokens[look_ahead];
                    if la.token_type == T::Newline || is_blank_text(la) {
                        look_ahead += 1;
                        continue;
                    }
                    break;
                }

                let newline_count = look_ahead - self.p;
                if opts.stop_at_paragraph_break
                    && newline_count >= 2
                    && look_ahead < self.tokens.len()
                    && self.tokens[look_ahead].token_type == T::TextLine
                    && !js::trim(self.tokens[look_ahead].value).is_empty()
                {
                    break;
                }

                if look_ahead < self.tokens.len()
                    && self.tokens[look_ahead].token_type == T::Annotation
                {
                    while self.p < look_ahead
                        && (self.check(T::Newline) || is_blank_text(&self.peek()))
                    {
                        if !opts.trim_trailing_newlines {
                            if self.check(T::Newline) {
                                value.push('\n');
                            }
                            end_pos = self.peek().end;
                        }
                        self.advance();
                    }
                    break;
                }

                if look_ahead >= self.tokens.len()
                    || self.tokens[look_ahead].token_type == T::EndOfInput
                {
                    while !self.is_at_end()
                        && (self.check(T::Newline) || is_blank_text(&self.peek()))
                    {
                        if !opts.trim_trailing_newlines {
                            if self.check(T::Newline) {
                                value.push('\n');
                            }
                            end_pos = self.peek().end;
                        }
                        self.advance();
                    }
                    break;
                }

                value.push('\n');
                end_pos = current.end;
                self.advance();
                continue;
            }

            if is_blank_text(&current) {
                self.advance();
                continue;
            }

            if value.is_empty() {
                let trimmed = js::trim_start(current.value);
                start_pos = current.start + (current.value.len() - trimmed.len()) as u32;
                value.push_str(trimmed);
            } else {
                value.push_str(current.value);
            }
            end_pos = current.end;
            self.advance();
        }

        Collected { value, start_pos, end_pos }
    }
}
