//! HTML elements, attributes, comments, doctype and dangling markers.

use super::factories::*;
use super::liquid_raw::has_liquid_open;
use super::parser::DocumentParser;
use super::tokenizer::{Token, TokenType};
use super::tree_builder::{compound_names_match, filter_children, ChildFilterMode};
use crate::ast::*;
use crate::errors::{Failure, PResult};
use crate::grammar::{BLOCKS, HTML_RAW_TAGS, VOID_ELEMENTS};
use crate::js;
use std::borrow::Cow;

fn is_branch_keyword(name: &str) -> bool {
    matches!(name, "else" | "elsif" | "when")
}

fn extract_plain_name<'s>(name: &[Node<'s>]) -> Option<&'s str> {
    match name {
        [Node::TextNode(t)] => match t.value {
            Cow::Borrowed(s) => Some(s),
            Cow::Owned(_) => None,
        },
        _ => None,
    }
}

/// `plainName || 'compound'`.
fn display_name(plain: Option<&str>) -> &str {
    match plain {
        Some(s) if !s.is_empty() => s,
        _ => "compound",
    }
}

fn is_left_trimming_open(token: Option<&Token>) -> bool {
    token.is_some_and(|t| {
        matches!(t.token_type, TokenType::LiquidVariableOutputOpen | TokenType::LiquidTagOpen)
            && t.end - t.start == 3
    })
}

fn raw_markup_kind_for_html_tag(
    tag_name: &str,
    attributes: &[Node<'_>],
    body_source: &str,
) -> RawMarkupKind {
    if js::lower_eq(tag_name, "script") {
        script_kind_from_attributes(attributes)
    } else if js::lower_eq(tag_name, "style") {
        if has_liquid_open(body_source) {
            RawMarkupKind::Text
        } else {
            RawMarkupKind::Css
        }
    } else {
        RawMarkupKind::Text
    }
}

fn script_kind_from_attributes(attributes: &[Node<'_>]) -> RawMarkupKind {
    let Some(type_value) = extract_plain_attribute_value(attributes, "type") else {
        return RawMarkupKind::Javascript;
    };
    if type_value == "text/html" {
        return RawMarkupKind::Html;
    }
    if type_value == "text/markdown" {
        return RawMarkupKind::Markdown;
    }
    if type_value.ends_with("json")
        || type_value.ends_with("importmap")
        || type_value == "speculationrules"
    {
        return RawMarkupKind::Json;
    }
    RawMarkupKind::Javascript
}

fn extract_plain_attribute_value<'a>(
    attributes: &'a [Node<'_>],
    target_name: &str,
) -> Option<&'a str> {
    for attr in attributes {
        let (Node::AttrSingleQuoted(a) | Node::AttrDoubleQuoted(a) | Node::AttrUnquoted(a)) = attr
        else {
            continue;
        };
        let Some(name) = extract_plain_name(&a.name) else { continue };
        if !js::lower_eq(name, target_name) {
            continue;
        }
        return match a.value.as_slice() {
            [Node::TextNode(t)] => Some(&t.value),
            _ => None,
        };
    }
    None
}

impl<'s> DocumentParser<'s> {
    // htmlElement := "<" compoundName attributes ("/" | ">") children? "</" compoundName ">"
    pub fn parse_html_element(&mut self) -> PResult<Node<'s>> {
        let source = self.source;
        let open_tag_start = self.peek().start;
        self.consume(TokenType::HtmlTagOpen)?;

        let name = self.parse_compound_name()?;
        let attributes = self.parse_attributes()?;

        if let Some(close) = self.accept(TokenType::HtmlSelfClose) {
            let block_start_position = Position::new(open_tag_start, close.end);
            return Ok(Node::HtmlSelfClosingElement(Box::new(HtmlSelfClosingElement {
                id: ID0,
                position: block_start_position,
                name,
                attributes,
                block_start_position,
            })));
        }

        let open_tag_end = self.consume(TokenType::HtmlTagClose)?.end;
        let block_start_position = Position::new(open_tag_start, open_tag_end);

        let plain_name = extract_plain_name(&name);
        if let Some(plain) = plain_name {
            if js::lower_in(plain, VOID_ELEMENTS) {
                return Ok(Node::HtmlVoidElement(Box::new(HtmlVoidElement {
                    id: ID0,
                    position: block_start_position,
                    name: plain,
                    attributes,
                    block_start_position,
                })));
            }
            if js::lower_in(plain, HTML_RAW_TAGS) {
                return self.parse_html_raw_body(plain, attributes, block_start_position);
            }
        }

        let mut children: Vec<Node<'s>> = Vec::new();
        while !self.is_at_end() {
            if self.check(TokenType::HtmlCloseTagOpen) {
                break;
            }
            if self.is_block_terminator() {
                if !self.allow_unclosed_html {
                    let tag_name = self.peek_tag_name().unwrap_or("");
                    return Err(Failure::parsing(
                        format!(
                            "Attempting to close LiquidTag '{tag_name}' before HtmlElement '{}' was closed",
                            display_name(plain_name)
                        ),
                        open_tag_start,
                        open_tag_end,
                    ));
                }
                break;
            }
            children.push(self.parse_node()?);
        }

        if self.is_at_end() || (self.allow_unclosed_html && self.is_block_terminator()) {
            if !self.allow_unclosed_html {
                return Err(Failure::parsing(
                    format!(
                        "Attempting to end parsing before HtmlElement '{}' was closed",
                        display_name(plain_name)
                    ),
                    open_tag_start,
                    open_tag_end,
                ));
            }
            let end_pos = children.last().map_or(open_tag_end, |c| c.position().end);
            let merged = filter_children(ChildFilterMode::Syntactic, children, source);
            return Ok(Self::make_html_element(
                name,
                attributes,
                merged,
                block_start_position,
                Position::new(end_pos, end_pos),
            ));
        }

        let close_start = self.peek().start;
        self.consume(TokenType::HtmlCloseTagOpen)?;
        let close_name = self.parse_compound_name()?;
        self.skip_blank_text_tokens();
        let close_end = self.consume(TokenType::HtmlTagClose)?.end;

        if !compound_names_match(&name, &close_name, source) {
            let close_plain_name = extract_plain_name(&close_name);
            return Err(Failure::parsing(
                format!(
                    "Attempting to close HtmlElement '{}' before HtmlElement '{}' was closed",
                    display_name(close_plain_name),
                    display_name(plain_name)
                ),
                close_start,
                close_end,
            ));
        }

        let merged = filter_children(ChildFilterMode::Syntactic, children, source);
        Ok(Self::make_html_element(
            name,
            attributes,
            merged,
            block_start_position,
            Position::new(close_start, close_end),
        ))
    }

    fn make_html_element(
        name: Vec<Node<'s>>,
        attributes: Vec<Node<'s>>,
        children: Vec<Node<'s>>,
        block_start_position: Position,
        block_end_position: Position,
    ) -> Node<'s> {
        Node::HtmlElement(Box::new(HtmlElement {
            id: ID0,
            position: Position::new(block_start_position.start, block_end_position.end),
            name,
            attributes,
            children,
            block_start_position,
            block_end_position,
        }))
    }

    /// `while (check(Text)) { if (/^\s*$/.test(text)) advance(); else break; }`
    fn skip_blank_text_tokens(&mut self) {
        while self.check(TokenType::Text) {
            let t = self.peek();
            if js::has_non_ws(js::slice(self.source, t.start, t.end)) {
                break;
            }
            self.advance();
        }
    }

    // htmlRawBody := (text | liquidVariableOutput | liquidTag)* "</" tagName ">"
    fn parse_html_raw_body(
        &mut self,
        plain_name: &'s str,
        attributes: Vec<Node<'s>>,
        block_start_position: Position,
    ) -> PResult<Node<'s>> {
        let source = self.source;
        let body_start = block_start_position.end;

        let Some(close_idx) = self.scan_for_html_close_tag(plain_name) else {
            return Err(Failure::parsing(
                format!("Attempting to end parsing before HtmlRawNode '{plain_name}' was closed"),
                block_start_position.start,
                block_start_position.end,
            ));
        };

        let body_end = self.tokens[close_idx].start;
        let body_string = js::slice(source, body_start, body_end);

        let body_nodes = self.parse_liquid_in_range(body_end)?;

        let kind = raw_markup_kind_for_html_tag(plain_name, &attributes, body_string);
        let body = make_raw_markup(kind, body_string, body_nodes, body_start, body_end);

        self.seek_to_source_offset(body_end);
        let close_start = self.peek().start;
        self.consume(TokenType::HtmlCloseTagOpen)?;
        while self.check(TokenType::Text) {
            self.advance();
        }
        let close_end = self.consume(TokenType::HtmlTagClose)?.end;

        let block_end_position = Position::new(close_start, close_end);
        Ok(Node::HtmlRawNode(Box::new(HtmlRawNode {
            id: ID0,
            position: Position::new(block_start_position.start, block_end_position.end),
            name: plain_name,
            attributes,
            body,
            block_start_position,
            block_end_position,
        })))
    }

    // htmlComment := "<!--" text* "-->"
    pub fn parse_html_comment(&mut self) -> PResult<Node<'s>> {
        let source = self.source;
        let open_token = self.consume(TokenType::HtmlCommentOpen)?;
        let body_start = open_token.end;

        let Some(found) = memchr::memmem::find(&source.as_bytes()[body_start as usize..], b"-->")
        else {
            return Err(Failure::parsing(
                "Attempting to end parsing before HtmlComment '<!--' was closed",
                open_token.start,
                open_token.end,
            ));
        };
        let body_end = body_start + found as u32;
        let comment_end = body_end + 3;
        let body = js::trim(js::slice(source, body_start, body_end));

        self.seek_to_source_offset(comment_end);

        Ok(Node::HtmlComment(Box::new(HtmlComment {
            id: ID0,
            position: Position::new(open_token.start, comment_end),
            body,
        })))
    }

    // htmlDoctype := "<!" text ">"
    pub fn parse_html_doctype(&mut self) -> PResult<Node<'s>> {
        let source = self.source;
        let open_token = self.consume(TokenType::HtmlDoctypeOpen)?;

        while !self.is_at_end() && !self.check(TokenType::HtmlTagClose) {
            self.advance();
        }

        let close_token = self.consume(TokenType::HtmlTagClose)?;
        let body = js::trim(js::slice(source, open_token.end, close_token.start));

        Ok(Node::HtmlDoctype(Box::new(HtmlDoctype {
            id: ID0,
            position: Position::new(open_token.start, close_token.end),
            legacy_doctype_string: legacy_doctype_string(body),
        })))
    }

    /// Always ends in a failure: either the close tag is malformed or it is
    /// an orphan.
    pub fn parse_orphaned_html_close_tag(&mut self) -> Failure {
        let close_start = self.peek().start;
        if let Err(e) = self.consume(TokenType::HtmlCloseTagOpen) {
            return e;
        }
        let close_name = match self.parse_compound_name() {
            Ok(name) => name,
            Err(e) => return e,
        };
        self.skip_blank_text_tokens();
        let close_end = match self.consume(TokenType::HtmlTagClose) {
            Ok(t) => t.end,
            Err(e) => return e,
        };
        Failure::parsing(
            format!(
                "Attempting to close HtmlElement '{}' before it was opened",
                display_name(extract_plain_name(&close_name))
            ),
            close_start,
            close_end,
        )
    }

    pub fn parse_html_dangling_marker_close(&mut self) -> PResult<Node<'s>> {
        let start = self.peek().start;
        self.consume(TokenType::HtmlCloseTagOpen)?;
        let name = self.parse_compound_name()?;
        self.skip_blank_text_tokens();
        let end = self.consume(TokenType::HtmlTagClose)?.end;
        let at = Position::new(start, end);
        Ok(Node::HtmlDanglingMarkerClose(Box::new(HtmlDanglingMarkerClose {
            id: ID0,
            position: at,
            name,
            block_start_position: at,
        })))
    }

    /// Attributes inside Liquid tag branches in attribute position, up to a block
    /// terminator.
    pub fn parse_branch_attributes(&mut self) -> PResult<Vec<Node<'s>>> {
        let mut attrs = Vec::new();
        while !self.is_at_end() {
            self.skip_space();
            if self.is_at_end() {
                break;
            }
            if self.check(TokenType::HtmlTagClose) || self.check(TokenType::HtmlSelfClose) {
                break;
            }
            if self.is_block_terminator() {
                break;
            }
            self.parse_one_attribute(&mut attrs)?;
        }
        Ok(attrs)
    }

    fn parse_one_attribute(&mut self, attrs: &mut Vec<Node<'s>>) -> PResult<()> {
        if self.check(TokenType::LiquidTagOpen) {
            // Not restored on failure, as upstream: only `parse_attributes`
            // resets this context in its `finally`.
            let saved = self.in_attribute_context;
            self.in_attribute_context = true;
            attrs.push(self.parse_liquid_tag()?);
            self.in_attribute_context = saved;
            return Ok(());
        }
        if self.check(TokenType::LiquidVariableOutputOpen) {
            attrs.push(self.parse_liquid_variable_output()?);
            return Ok(());
        }
        let attr_start = self.peek().start;
        let name = self.parse_compound_name()?;
        if name.is_empty() {
            self.advance();
            return Ok(());
        }
        attrs.push(self.parse_attribute_after_name(name, attr_start)?);
        Ok(())
    }

    // compoundName := (text | liquidVariableOutput | liquidTag)+
    fn parse_compound_name(&mut self) -> PResult<Vec<Node<'s>>> {
        let mut segments: Vec<Node<'s>> = Vec::with_capacity(1);
        while !self.is_at_end() {
            if self.check(TokenType::LiquidVariableOutputOpen) {
                segments.push(self.parse_liquid_variable_output()?);
                continue;
            }
            if !self.in_attribute_context
                && self.check(TokenType::LiquidTagOpen)
                && !segments.iter().any(|s| matches!(s, Node::LiquidVariableOutput(_)))
                && !self.liquid_tag_starts_attribute_list(&segments)
            {
                segments.push(self.parse_liquid_tag()?);
                continue;
            }
            if let Some(segment) = self.consume_text_segment() {
                segments.push(segment);
                continue;
            }
            break;
        }
        Ok(segments)
    }

    // attributes := (attribute | liquidVariableOutput | liquidTag)*
    fn parse_attributes(&mut self) -> PResult<Vec<Node<'s>>> {
        let saved = self.in_attribute_context;
        self.in_attribute_context = true;
        let result = self.parse_attribute_list();
        self.in_attribute_context = saved;
        result
    }

    fn parse_attribute_list(&mut self) -> PResult<Vec<Node<'s>>> {
        let mut attrs = Vec::new();
        while !self.is_at_end() {
            self.skip_space();
            if self.is_at_end() {
                break;
            }
            if self.check(TokenType::HtmlTagClose) || self.check(TokenType::HtmlSelfClose) {
                break;
            }
            self.parse_one_attribute(&mut attrs)?;
        }
        Ok(attrs)
    }

    // attribute := compoundName (space* "=" space* (quotedAttrValue | unquotedAttrValue))?
    fn parse_attribute_after_name(
        &mut self,
        name: Vec<Node<'s>>,
        attr_start: u32,
    ) -> PResult<Node<'s>> {
        let source = self.source;
        self.skip_space();
        if self.accept(TokenType::HtmlEquals).is_none() {
            let end = name[name.len() - 1].position().end;
            return Ok(Node::AttrEmpty(Box::new(AttrEmpty {
                id: ID0,
                position: Position::new(attr_start, end),
                name,
            })));
        }
        self.skip_space();

        let Some(quote_token) = self.accept(TokenType::HtmlQuoteOpen) else {
            let (value, value_end) = self.parse_unquoted_attribute_value()?;
            let attribute_position = Position::new(
                value.first().map_or_else(|| self.peek().start, |v| v.position().start),
                value_end,
            );
            return Ok(Node::AttrUnquoted(Box::new(AttrNode {
                id: ID0,
                position: Position::new(attr_start, value_end),
                name,
                value,
                attribute_position,
            })));
        };

        let quote_char = source[quote_token.start as usize..].chars().next().unwrap_or('"');
        let value_start = quote_token.end;
        let value = self.parse_quoted_attribute_value()?;
        let close_quote = self.consume(TokenType::HtmlQuoteClose)?;
        let attr = Box::new(AttrNode {
            id: ID0,
            position: Position::new(attr_start, close_quote.end),
            name,
            value,
            attribute_position: Position::new(value_start, close_quote.start),
        });
        Ok(if matches!(quote_char, '"' | '\u{201C}' | '\u{201D}') {
            Node::AttrDoubleQuoted(attr)
        } else {
            Node::AttrSingleQuoted(attr)
        })
    }

    #[inline]
    fn skip_space(&mut self) {
        while self.check(TokenType::Text) && self.is_space_token(&self.peek()) {
            self.advance();
        }
    }

    #[inline]
    fn is_space_token(&self, token: &Token) -> bool {
        js::ws_width_at(self.source, token.start as usize) > 0
    }

    // quotedAttrValue := (text | liquidVariableOutput | liquidTag)*
    fn parse_quoted_attribute_value(&mut self) -> PResult<Vec<Node<'s>>> {
        let mut values = Vec::new();
        let source = self.source;

        while !self.is_at_end() && !self.check(TokenType::HtmlQuoteClose) {
            if self.check(TokenType::LiquidVariableOutputOpen) {
                values.push(self.parse_liquid_variable_output()?);
            } else if self.check(TokenType::LiquidTagOpen) {
                if let Some(tag_name) = self.peek_tag_name() {
                    if !tag_name.is_empty()
                        && (is_branch_keyword(tag_name) || tag_name.starts_with("end"))
                    {
                        let token = self.peek();
                        return Err(Failure::parsing(
                            format!(
                                "Unexpected Liquid tag '{tag_name}' inside quoted attribute value"
                            ),
                            token.start,
                            token.end,
                        ));
                    }
                }
                // Not restored on failure, as upstream.
                let saved = self.in_attribute_value_context;
                self.in_attribute_value_context = true;
                values.push(self.parse_liquid_tag()?);
                self.in_attribute_value_context = saved;
            } else if self.check(TokenType::Text) {
                let token = self.advance();
                values.push(make_text_node(
                    js::slice(source, token.start, token.end),
                    token.start,
                    token.end,
                ));
            } else {
                break;
            }
        }
        Ok(values)
    }

    // unquotedAttrValue := (text | liquidVariableOutput)+
    fn parse_unquoted_attribute_value(&mut self) -> PResult<(Vec<Node<'s>>, u32)> {
        let mut values = Vec::new();
        let mut end = self.peek().start;

        while !self.is_at_end() {
            if self.check(TokenType::HtmlTagClose) || self.check(TokenType::HtmlSelfClose) {
                break;
            }
            if let Some(segment) = self.consume_text_segment() {
                end = segment.position().end;
                values.push(segment);
                continue;
            }
            if self.check(TokenType::LiquidVariableOutputOpen) {
                let drop = self.parse_liquid_variable_output()?;
                end = drop.position().end;
                values.push(drop);
                continue;
            }
            break;
        }
        Ok((values, end))
    }

    /// The current Text token as a name or unquoted-value segment; a whitespace
    /// token is a separator and is left for the caller.
    #[inline]
    fn consume_text_segment(&mut self) -> Option<Node<'s>> {
        if !self.check(TokenType::Text) {
            return None;
        }
        let token = self.peek();
        if self.is_space_token(&token) {
            return None;
        }
        self.advance();
        Some(make_text_node(
            js::slice(self.source, token.start, token.end),
            token.start,
            token.end,
        ))
    }

    fn liquid_tag_starts_attribute_list(&self, segments: &[Node<'s>]) -> bool {
        if !segments.iter().any(|s| matches!(s, Node::TextNode(_))) {
            return false;
        }
        match self.peek_tag_name() {
            Some(name) if BLOCKS.contains(&name) => self.liquid_tag_body_starts_with_whitespace(),
            _ => false,
        }
    }

    fn liquid_tag_body_starts_with_whitespace(&self) -> bool {
        let source = self.source;
        let mut in_tag = false;
        let mut trim_next = false;
        for i in self.p..self.tokens.len() {
            let tok = self.tokens[i];
            match tok.token_type {
                TokenType::LiquidTagOpen => in_tag = true,
                TokenType::LiquidTagClose => {
                    in_tag = false;
                    trim_next = tok.end - tok.start == 3;
                }
                TokenType::Text => {
                    if !in_tag {
                        let mut text = js::slice(source, tok.start, tok.end);
                        if trim_next {
                            text = js::trim_start(text);
                        }
                        trim_next = false;
                        if text.is_empty() {
                            continue;
                        }
                        if js::is_all_ws_nonempty(text)
                            && is_left_trimming_open(self.tokens.get(i + 1))
                        {
                            continue;
                        }
                        return js::ws_width_at(text, 0) > 0;
                    }
                }
                TokenType::LiquidVariableOutputOpen
                | TokenType::HtmlTagClose
                | TokenType::HtmlSelfClose
                | TokenType::EndOfInput
                    if !in_tag =>
                {
                    return false;
                }
                _ => {}
            }
        }
        false
    }

    fn scan_for_html_close_tag(&self, tag_name: &str) -> Option<usize> {
        let lower_name = tag_name.to_lowercase();
        let source = self.source;
        let count = self.tokens.len();
        let mut depth = 0u32;
        let mut i = self.p;
        while i < count {
            let token = self.tokens[i];
            if token.token_type == TokenType::HtmlTagOpen {
                let text_idx = i + 1;
                if text_idx < count && self.tokens[text_idx].token_type == TokenType::Text {
                    let t = self.tokens[text_idx];
                    let trimmed = js::trim_start(js::slice(source, t.start, t.end));
                    let name = match js::find_ws(trimmed) {
                        None => trimmed,
                        Some(w) => &trimmed[..w],
                    };
                    if js::lower_eq(name, &lower_name) {
                        depth += 1;
                    }
                }
                i += 1;
                continue;
            }
            if token.token_type != TokenType::HtmlCloseTagOpen {
                i += 1;
                continue;
            }
            let mut end = i + 1;
            while end < count && self.tokens[end].token_type == TokenType::Text {
                end += 1;
            }
            if end == i + 1 {
                i += 1;
                continue;
            }
            let text = js::slice(source, token.end, self.tokens[end - 1].end);
            if js::lower_eq(js::trim(text), &lower_name) {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            i += 1;
        }
        None
    }
}

/// `body.match(/^doctype\s+html\s*(.*)/i)`, then `match ? (m[1] || null) : body`.
fn legacy_doctype_string(body: &str) -> Option<&str> {
    fn matched(body: &str) -> Option<&str> {
        let head = body.get(..7)?;
        if !head.eq_ignore_ascii_case("doctype") {
            return None;
        }
        let after = &body[7..];
        let html_at = js::trim_start(after);
        if html_at.len() == after.len() {
            return None;
        }
        let head = html_at.get(..4)?;
        if !head.eq_ignore_ascii_case("html") {
            return None;
        }
        let rest = js::trim_start(&html_at[4..]);
        // `.` stops at any line terminator.
        let end = rest.find(['\n', '\r', '\u{2028}', '\u{2029}']).unwrap_or(rest.len());
        Some(&rest[..end])
    }
    match matched(body) {
        Some("") => None,
        Some(m) => Some(m),
        None => Some(body),
    }
}
