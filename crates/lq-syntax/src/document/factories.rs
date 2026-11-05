//! Node constructors. Ids stay 0 until the post-parse numbering pass.

use super::tokenizer::Token;
use crate::ast::*;
use crate::js;
use std::borrow::Cow;

pub(crate) const ID0: NodeId = NodeId(0);

pub(crate) struct DelimiterWhitespace {
    pub start: WhitespaceMarker,
    pub end: WhitespaceMarker,
}

#[inline]
pub(crate) fn marker(token: &Token) -> WhitespaceMarker {
    if token.end - token.start > 2 {
        "-"
    } else {
        ""
    }
}

#[derive(Clone, Copy)]
pub(crate) struct LiquidTagEnvelope<'s> {
    pub tag_name: &'s str,
    pub markup_string: &'s str,
    pub markup_offset: u32,
    pub markup_end: u32,
    pub whitespace_start: WhitespaceMarker,
    pub whitespace_end: WhitespaceMarker,
    pub block_start_position: Position,
    pub source: &'s str,
}

pub(crate) fn make_document_node<'s>(children: Vec<Node<'s>>, source: &'s str) -> Node<'s> {
    Node::Document(Box::new(DocumentNode {
        id: ID0,
        position: Position::new(0, source.len() as u32),
        children,
        name: "#document",
        source,
    }))
}

pub(crate) fn envelope_from_tokens<'s>(
    open_token: &Token,
    close_token: &Token,
    source: &'s str,
) -> LiquidTagEnvelope<'s> {
    let inner = js::slice(source, open_token.end, close_token.start);
    let trimmed = js::trim_start(inner);
    let leading_spaces = inner.len() - trimmed.len();

    let tag_name = match js::find_ws(trimmed) {
        None => trimmed,
        Some(i) => &trimmed[..i],
    };

    let after_tag_name = leading_spaces + tag_name.len();
    let markup_string = &inner[after_tag_name..];

    let bytes = inner.as_bytes();
    let mut markup_offset_in_inner = after_tag_name;
    while markup_offset_in_inner < bytes.len()
        && (bytes[markup_offset_in_inner] == b' ' || bytes[markup_offset_in_inner] == b'\t')
    {
        markup_offset_in_inner += 1;
    }

    LiquidTagEnvelope {
        tag_name,
        markup_string,
        markup_offset: open_token.end + markup_offset_in_inner as u32,
        markup_end: close_token.start,
        whitespace_start: marker(open_token),
        whitespace_end: marker(close_token),
        block_start_position: Position::new(open_token.start, close_token.end),
        source,
    }
}

pub(crate) fn envelope_from_line<'s>(
    line: &crate::tags::LiquidLine<'s>,
    source: &'s str,
) -> LiquidTagEnvelope<'s> {
    LiquidTagEnvelope {
        tag_name: line.tag_name,
        markup_string: line.markup,
        markup_offset: line.markup_offset,
        markup_end: line.markup_offset + line.markup.len() as u32,
        whitespace_start: "",
        whitespace_end: "",
        block_start_position: Position::new(line.name_offset, line.line_end),
        source,
    }
}

#[derive(Default)]
pub(crate) struct TagBody<'s> {
    pub children: Option<Vec<Node<'s>>>,
    pub block_end_position: Option<Position>,
    pub delimiter_whitespace: Option<DelimiterWhitespace>,
}

pub(crate) fn make_liquid_tag_base_case<'s>(
    envelope: &LiquidTagEnvelope<'s>,
    body: TagBody<'s>,
    reason: Option<Cow<'s, str>>,
    preserve_markup: bool,
) -> Node<'s> {
    let markup = if preserve_markup {
        envelope.markup_string
    } else {
        js::trim(envelope.markup_string)
    };
    make_liquid_tag(envelope, Markup::Str(markup), body, reason)
}

pub(crate) fn make_liquid_tag_named<'s>(
    envelope: &LiquidTagEnvelope<'s>,
    markup: Markup<'s>,
    body: TagBody<'s>,
) -> Node<'s> {
    make_liquid_tag(envelope, markup, body, None)
}

fn make_liquid_tag<'s>(
    envelope: &LiquidTagEnvelope<'s>,
    markup: Markup<'s>,
    body: TagBody<'s>,
    reason: Option<Cow<'s, str>>,
) -> Node<'s> {
    let pos_end = body.block_end_position.map_or(envelope.block_start_position.end, |p| p.end);
    Node::LiquidTag(Box::new(LiquidTag {
        id: ID0,
        position: Position::new(envelope.block_start_position.start, pos_end),
        name: envelope.tag_name,
        markup,
        children: body.children,
        whitespace_start: envelope.whitespace_start,
        whitespace_end: envelope.whitespace_end,
        delimiter_whitespace_start: body.delimiter_whitespace.as_ref().map(|w| w.start),
        delimiter_whitespace_end: body.delimiter_whitespace.as_ref().map(|w| w.end),
        block_start_position: envelope.block_start_position,
        block_end_position: body.block_end_position,
        markup_position: Position::new(envelope.markup_offset, envelope.markup_end),
        reason,
    }))
}

pub(crate) fn make_liquid_branch_unnamed<'s>(body_start: u32) -> LiquidBranch<'s> {
    let at = Position::new(body_start, body_start);
    LiquidBranch {
        id: ID0,
        position: at,
        name: None,
        markup: Markup::Str(""),
        children: Vec::new(),
        whitespace_start: "",
        whitespace_end: "",
        markup_position: at,
        block_start_position: at,
        block_end_position: Position::new(0, 0),
    }
}

pub(crate) fn make_liquid_branch_named<'s>(
    envelope: &LiquidTagEnvelope<'s>,
    markup: Markup<'s>,
) -> LiquidBranch<'s> {
    LiquidBranch {
        id: ID0,
        position: envelope.block_start_position,
        name: Some(envelope.tag_name),
        markup,
        children: Vec::new(),
        whitespace_start: envelope.whitespace_start,
        whitespace_end: envelope.whitespace_end,
        markup_position: Position::new(envelope.markup_offset, envelope.markup_end),
        block_start_position: envelope.block_start_position,
        block_end_position: Position::new(0, 0),
    }
}

pub(crate) fn make_liquid_raw_tag<'s>(
    envelope: &LiquidTagEnvelope<'s>,
    body: Node<'s>,
    block_end_position: Position,
    delimiter_whitespace: DelimiterWhitespace,
) -> Node<'s> {
    let bytes = envelope.source.as_bytes();
    let mut markup_end = envelope.markup_end;
    while markup_end > envelope.markup_offset
        && (bytes[markup_end as usize - 1] == b' ' || bytes[markup_end as usize - 1] == b'\t')
    {
        markup_end -= 1;
    }
    Node::LiquidRawTag(Box::new(LiquidRawTag {
        id: ID0,
        position: Position::new(envelope.block_start_position.start, block_end_position.end),
        name: envelope.tag_name,
        markup: js::trim(envelope.markup_string),
        markup_position: Position::new(envelope.markup_offset, markup_end),
        body,
        whitespace_start: envelope.whitespace_start,
        whitespace_end: envelope.whitespace_end,
        delimiter_whitespace_start: delimiter_whitespace.start,
        delimiter_whitespace_end: delimiter_whitespace.end,
        block_start_position: envelope.block_start_position,
        block_end_position,
    }))
}

pub(crate) fn make_liquid_variable_output<'s>(
    open_token: &Token,
    close_token: &Token,
    markup: Markup<'s>,
) -> Node<'s> {
    Node::LiquidVariableOutput(Box::new(LiquidVariableOutput {
        id: ID0,
        position: Position::new(open_token.start, close_token.end),
        markup,
        whitespace_start: marker(open_token),
        whitespace_end: marker(close_token),
        markup_position: Position::new(open_token.end, close_token.start),
    }))
}

#[inline]
pub(crate) fn make_text_node<'s>(value: impl Into<Cow<'s, str>>, start: u32, end: u32) -> Node<'s> {
    Node::TextNode(Box::new(TextNode {
        id: ID0,
        position: Position::new(start, end),
        value: value.into(),
    }))
}

pub(crate) fn make_raw_markup<'s>(
    kind: RawMarkupKind,
    value: &'s str,
    nodes: Vec<Node<'s>>,
    start: u32,
    end: u32,
) -> Node<'s> {
    Node::RawMarkup(Box::new(RawMarkup {
        id: ID0,
        position: Position::new(start, end),
        kind,
        value,
        nodes,
    }))
}
