//! Tag definitions: `builtinTags`, with one parse arm per tag.

use crate::ast::*;
use crate::errors::{Failure, PResult};
use crate::js;
use crate::markup::parser::MarkupParser;
use crate::markup::tokenizer::MarkupTokenType as T;

const ID0: NodeId = NodeId(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagKind {
    Block,
    Tag,
    Raw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagId {
    Assign,
    Block,
    Break,
    Capture,
    Case,
    ContentFor,
    Continue,
    Cycle,
    Decrement,
    Echo,
    For,
    Form,
    If,
    Ifchanged,
    Include,
    Increment,
    Layout,
    Liquid,
    Paginate,
    Partial,
    Render,
    Section,
    Sections,
    Tablerow,
    Unless,
    Comment,
    Doc,
    Javascript,
    Raw,
    Schema,
    Style,
    Stylesheet,
}

/// The result of `Environment.tagForName`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagDefinition {
    Block { id: TagId, branches: &'static [&'static str] },
    Tag { id: TagId },
    Raw { id: TagId, parse_liquid_in_body: bool },
    /// A name that `builtins[name]` resolves through `Object.prototype`:
    /// truthy with no `kind`, so dispatching on it throws.
    ObjectPrototype,
}

/// The message `assertNever(def)` throws for `TagDefinition::ObjectPrototype`.
pub(crate) const ASSERT_NEVER_MESSAGE: &str = "Unexpected object: undefined";

impl TagDefinition {
    pub fn kind(&self) -> Option<TagKind> {
        match self {
            TagDefinition::Block { .. } => Some(TagKind::Block),
            TagDefinition::Tag { .. } => Some(TagKind::Tag),
            TagDefinition::Raw { .. } => Some(TagKind::Raw),
            TagDefinition::ObjectPrototype => None,
        }
    }
}

pub fn tag_for_name(name: &str) -> Option<TagDefinition> {
    const IF_BRANCHES: &[&str] = &["elsif", "else"];
    const CASE_BRANCHES: &[&str] = &["when", "else"];
    const FOR_BRANCHES: &[&str] = &["else"];
    const NONE: &[&str] = &[];
    use TagDefinition as D;
    use TagId as I;
    Some(match name {
        "assign" => D::Tag { id: I::Assign },
        "block" => D::Block { id: I::Block, branches: NONE },
        "break" => D::Tag { id: I::Break },
        "capture" => D::Block { id: I::Capture, branches: NONE },
        "case" => D::Block { id: I::Case, branches: CASE_BRANCHES },
        "content_for" => D::Tag { id: I::ContentFor },
        "continue" => D::Tag { id: I::Continue },
        "cycle" => D::Tag { id: I::Cycle },
        "decrement" => D::Tag { id: I::Decrement },
        "echo" => D::Tag { id: I::Echo },
        "for" => D::Block { id: I::For, branches: FOR_BRANCHES },
        "form" => D::Block { id: I::Form, branches: NONE },
        "if" => D::Block { id: I::If, branches: IF_BRANCHES },
        "ifchanged" => D::Block { id: I::Ifchanged, branches: NONE },
        "include" => D::Tag { id: I::Include },
        "increment" => D::Tag { id: I::Increment },
        "layout" => D::Tag { id: I::Layout },
        "liquid" => D::Tag { id: I::Liquid },
        "paginate" => D::Block { id: I::Paginate, branches: NONE },
        "partial" => D::Block { id: I::Partial, branches: NONE },
        "render" => D::Tag { id: I::Render },
        "section" => D::Tag { id: I::Section },
        "sections" => D::Tag { id: I::Sections },
        "tablerow" => D::Block { id: I::Tablerow, branches: FOR_BRANCHES },
        "unless" => D::Block { id: I::Unless, branches: IF_BRANCHES },
        "comment" => D::Raw { id: I::Comment, parse_liquid_in_body: false },
        "doc" => D::Raw { id: I::Doc, parse_liquid_in_body: false },
        "javascript" => D::Raw { id: I::Javascript, parse_liquid_in_body: true },
        "raw" => D::Raw { id: I::Raw, parse_liquid_in_body: false },
        "schema" => D::Raw { id: I::Schema, parse_liquid_in_body: false },
        "style" => D::Raw { id: I::Style, parse_liquid_in_body: true },
        "stylesheet" => D::Raw { id: I::Stylesheet, parse_liquid_in_body: true },
        "constructor" | "__defineGetter__" | "__defineSetter__" | "hasOwnProperty"
        | "__lookupGetter__" | "__lookupSetter__" | "isPrototypeOf" | "propertyIsEnumerable"
        | "toString" | "valueOf" | "__proto__" | "toLocaleString" => D::ObjectPrototype,
        _ => return None,
    })
}

/// A parsed line from a `{% liquid %}` body.
#[derive(Clone, Copy, Debug)]
pub struct LiquidLine<'s> {
    pub tag_name: &'s str,
    pub markup: &'s str,
    pub markup_offset: u32,
    pub name_offset: u32,
    pub line_end: u32,
}

/// Shared iterator over the lines of a `{% liquid %}` body.
#[derive(Clone, Debug)]
pub struct LiquidLineContext<'s> {
    pub lines: Vec<LiquidLine<'s>>,
    pub index: usize,
}

/// Upstream's `Parser` delegate: only `liquid` calls back through it.
pub trait StatementParser<'s> {
    fn parse_liquid_statement(
        &mut self,
        tag_name: &'s str,
        markup_string: &'s str,
        markup_offset: u32,
        ctx: &mut LiquidLineContext<'s>,
    ) -> PResult<Node<'s>>;
}

fn string_only<'s>(expr: Node<'s>, what: &str) -> PResult<Node<'s>> {
    if !matches!(expr, Node::String(_)) {
        return Err(Failure::error(format!(
            "Expected string for {what}, got {}",
            expr.node_type()
        )));
    }
    Ok(expr)
}

fn boxed(node: Node<'_>) -> Markup<'_> {
    Markup::Node(Box::new(node))
}

/// `def.parse(name, markup, parser)`.
pub fn parse_tag<'s>(
    tag: TagId,
    name: &str,
    markup: &mut MarkupParser<'s>,
    parser: &mut dyn StatementParser<'s>,
) -> PResult<Markup<'s>> {
    match tag {
        TagId::Assign => parse_assign(markup).map(boxed),
        TagId::Block => parse_block(markup).map(boxed),
        TagId::Break | TagId::Continue => {
            if !markup.is_at_end() {
                return Err(Failure::error(format!("'{name}' tag does not accept markup")));
            }
            Ok(Markup::Str(""))
        }
        TagId::Capture => parse_capture(markup).map(boxed),
        TagId::Case | TagId::Layout => markup.value_expression().map(boxed),
        TagId::ContentFor => parse_content_for(markup).map(boxed),
        TagId::Cycle => parse_cycle(markup).map(boxed),
        TagId::Decrement | TagId::Increment => markup.variable_lookup().map(boxed),
        TagId::Echo => markup.liquid_variable().map(boxed),
        TagId::For | TagId::Tablerow => parse_for(markup).map(boxed),
        TagId::Form => markup.arguments().map(Markup::List),
        TagId::If | TagId::Unless => markup.conditional_expression().map(boxed),
        TagId::Ifchanged => Ok(Markup::None),
        TagId::Include => parse_render(markup, true).map(boxed),
        TagId::Liquid => parse_liquid(markup, parser).map(Markup::List),
        TagId::Paginate => parse_paginate(markup).map(boxed),
        TagId::Partial => string_only(markup.value_expression()?, "partial name").map(boxed),
        TagId::Render => parse_render(markup, false).map(boxed),
        TagId::Section => parse_section(markup).map(boxed),
        TagId::Sections => string_only(markup.value_expression()?, "sections name").map(boxed),
        TagId::Comment
        | TagId::Doc
        | TagId::Javascript
        | TagId::Raw
        | TagId::Schema
        | TagId::Style
        | TagId::Stylesheet => {
            if !markup.is_at_end() {
                return Err(Failure::error(format!("'{name}' tag does not accept markup")));
            }
            Ok(Markup::None)
        }
    }
}

fn parse_assign<'s>(markup: &mut MarkupParser<'s>) -> PResult<Node<'s>> {
    let start = markup.peek().start;
    let name: &'s str = if markup.is_lax() {
        let raw = markup.consume_raw_until(T::Equality);
        // The last match of /[\w\-.[\]()]+/g.
        let is_sig = |b: u8| js::is_word_byte(b) || matches!(b, b'-' | b'.' | b'[' | b']' | b'(' | b')');
        let bytes = raw.as_bytes();
        let mut end = bytes.len();
        while end > 0 && !is_sig(bytes[end - 1]) {
            end -= 1;
        }
        let mut begin = end;
        while begin > 0 && is_sig(bytes[begin - 1]) {
            begin -= 1;
        }
        if begin == end {
            return Err(Failure::error("Expected an assignment target before '=' in assign tag"));
        }
        &raw[begin..end]
    } else {
        markup.consume(T::Id)?.value
    };
    let eq_token = markup.consume(T::Equality)?;
    if eq_token.value != "=" {
        return Err(Failure::error("Expected '=' in assign tag"));
    }
    let value = markup.liquid_variable()?;
    Ok(Node::AssignMarkup(Box::new(AssignMarkup {
        id: ID0,
        position: Position::new(start, markup.peek().start),
        name,
        value,
    })))
}

/// `/^_?[a-zA-Z0-9][\w-]*$/`.
fn is_block_type(s: &str) -> bool {
    let b = s.as_bytes();
    let i = usize::from(b.first() == Some(&b'_'));
    b.get(i).is_some_and(u8::is_ascii_alphanumeric)
        && b[i + 1..].iter().all(|&c| js::is_word_byte(c) || c == b'-')
}

/// `[A-Za-z0-9][A-Za-z0-9_-]*`.
fn is_handle(s: &str) -> bool {
    let b = s.as_bytes();
    b.first().is_some_and(u8::is_ascii_alphanumeric)
        && b.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// `/^shopify:\/\/apps\/HANDLE\/blocks\/HANDLE\/UUID$/`.
fn is_app_block_type(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("shopify://apps/") else { return false };
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() != 4 || parts[1] != "blocks" || !is_handle(parts[0]) || !is_handle(parts[2]) {
        return false;
    }
    let groups: Vec<&str> = parts[3].split('-').collect();
    let lens = [8usize, 4, 4, 4, 12];
    groups.len() == 5
        && groups
            .iter()
            .zip(lens)
            .all(|(g, l)| g.len() == l && g.bytes().all(|c| c.is_ascii_hexdigit()))
}

fn parse_block<'s>(markup: &mut MarkupParser<'s>) -> PResult<Node<'s>> {
    if markup.is_at_end() {
        return Err(Failure::error("in 'block' - missing block type"));
    }
    let name = markup.value_expression()?;
    let Node::String(string) = &name else {
        return Err(Failure::error("in 'block' - file name must be a string literal"));
    };
    if !is_block_type(string.value) && !is_app_block_type(string.value) {
        return Err(Failure::error(format!(
            "in 'block' - '{}' is not a valid block type",
            string.value
        )));
    }
    let mut args = Vec::new();
    if markup.consume_optional(T::Comma).is_some() && !markup.is_at_end() {
        args = markup.block_named_arguments()?;
    }
    if !markup.is_at_end() {
        return Err(Failure::error(format!(
            "Unexpected token in 'block' tag: {}",
            markup.peek().value
        )));
    }
    Ok(Node::BlockMarkup(Box::new(BlockMarkup {
        id: ID0,
        position: Position::new(name.position().start, markup.peek().start),
        name,
        args,
    })))
}

/// `/((?:\(?[\w\-.[\]]\)?)+)/`: the first match, if any.
fn capture_name(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let is_sig = |c: u8| js::is_word_byte(c) || matches!(c, b'-' | b'.' | b'[' | b']');
    let unit = |i: usize| -> Option<usize> {
        let mut j = i;
        if b.get(j) == Some(&b'(') {
            j += 1;
        }
        if !b.get(j).copied().is_some_and(is_sig) {
            return None;
        }
        j += 1;
        if b.get(j) == Some(&b')') {
            j += 1;
        }
        Some(j)
    };
    for start in 0..b.len() {
        if let Some(mut end) = unit(start) {
            while let Some(next) = unit(end) {
                end = next;
            }
            return s.get(start..end);
        }
    }
    None
}

fn parse_capture<'s>(markup: &mut MarkupParser<'s>) -> PResult<Node<'s>> {
    if markup.is_lax() {
        let ctx = markup.body_context();
        if let Some(name) = capture_name(js::slice(ctx.source, ctx.body_start, ctx.body_end)) {
            return Ok(Node::VariableLookup(Box::new(LiquidVariableLookup {
                id: ID0,
                position: Position::new(ctx.body_start, ctx.body_end),
                name: Some(name),
                lookups: Vec::new(),
                lookup_modes: None,
            })));
        }
    }
    markup.variable_lookup()
}

/// `whenBranchParse`.
pub fn when_branch_parse<'s>(markup: &mut MarkupParser<'s>) -> PResult<Vec<Node<'s>>> {
    let mut values = vec![markup.value_expression()?];
    loop {
        if markup.consume_optional(T::Comma).is_some() {
            if markup.is_lax() && !markup.at_value_start() {
                break;
            }
            values.push(markup.value_expression()?);
            continue;
        }
        let token = markup.peek();
        if token.token_type == T::Logical && token.value == "or" {
            markup.consume(T::Logical)?;
            if markup.is_lax() && !markup.at_value_start() {
                break;
            }
            values.push(markup.value_expression()?);
            continue;
        }
        break;
    }
    let trailing_colon = markup.peek();
    if let Some(last_value @ Node::String(_)) = values.last() {
        if trailing_colon.token_type == T::Colon
            && markup.look(T::EndOfString, 1)
            && last_value.position().end == trailing_colon.start
        {
            markup.consume(T::Colon)?;
        }
    }
    if markup.is_lax() && !markup.is_at_end() {
        markup.discard_trailing();
    }
    Ok(values)
}

/// `elsifBranchParse`.
pub fn elsif_branch_parse<'s>(markup: &mut MarkupParser<'s>) -> PResult<Node<'s>> {
    markup.conditional_expression()
}

fn parse_content_for<'s>(markup: &mut MarkupParser<'s>) -> PResult<Node<'s>> {
    let content_for_type = string_only(markup.value_expression()?, "content_for type")?;
    let mut args = Vec::new();
    markup.consume_optional(T::Comma);
    while markup.look(T::Id, 0) {
        match markup.named_argument() {
            Ok(arg) => args.push(arg),
            Err(_) => break,
        }
        markup.consume_optional(T::Comma);
    }
    Ok(Node::ContentForMarkup(Box::new(ContentForMarkup {
        id: ID0,
        position: Position::new(content_for_type.position().start, markup.peek().start),
        content_for_type,
        args,
    })))
}

fn parse_cycle<'s>(markup: &mut MarkupParser<'s>) -> PResult<Node<'s>> {
    let first = markup.value_expression()?;
    let start = first.position().start;
    let mut group_name = None;
    let mut args = Vec::new();
    if markup.consume_optional(T::Colon).is_some() {
        group_name = Some(first);
        args.push(markup.value_expression()?);
    } else {
        args.push(first);
    }
    while markup.consume_optional(T::Comma).is_some() {
        if markup.is_lax() && markup.is_at_end() {
            break;
        }
        args.push(markup.value_expression()?);
    }
    Ok(Node::CycleMarkup(Box::new(CycleMarkup {
        id: ID0,
        position: Position::new(start, markup.peek().start),
        group_name,
        args,
    })))
}

fn parse_for<'s>(markup: &mut MarkupParser<'s>) -> PResult<Node<'s>> {
    let name_token = markup.consume(T::Id)?;
    if !markup.id("in") {
        return Err(Failure::error("Expected 'in'"));
    }
    let collection = markup.value_expression()?;
    let reversed = markup.id("reversed");
    let mut args = Vec::new();
    while !markup.is_at_end() {
        markup.consume_optional(T::Comma);
        if markup.is_lax() {
            match markup.named_argument() {
                Ok(arg) => args.push(arg),
                Err(_) => break,
            }
        } else {
            args.push(markup.named_argument()?);
        }
    }
    Ok(Node::ForMarkup(Box::new(ForMarkup {
        id: ID0,
        position: Position::new(name_token.start, markup.peek().start),
        variable_name: name_token.value,
        collection,
        reversed,
        args,
    })))
}

fn parse_paginate<'s>(markup: &mut MarkupParser<'s>) -> PResult<Node<'s>> {
    let collection = markup.value_expression()?;
    if !markup.id("by") {
        return Err(Failure::error("Expected 'by' in paginate tag"));
    }
    let page_size = markup.value_expression()?;
    let mut args = Vec::new();
    if markup.consume_optional(T::Comma).is_some()
        || (markup.is_tolerant() && markup.peek().token_type == T::Id)
    {
        args = markup.named_arguments()?;
    }
    Ok(Node::PaginateMarkup(Box::new(PaginateMarkup {
        id: ID0,
        position: Position::new(collection.position().start, markup.peek().start),
        collection,
        page_size,
        args,
    })))
}

fn parse_render<'s>(markup: &mut MarkupParser<'s>, allow_any_template_name: bool) -> PResult<Node<'s>> {
    let snippet = markup.value_expression()?;
    if !allow_any_template_name
        && !matches!(snippet, Node::String(_) | Node::VariableLookup(_))
    {
        return Err(Failure::error(format!(
            "Expected string or variable lookup for snippet, got {}",
            snippet.node_type()
        )));
    }

    let mut variable = None;
    let mut alias = None;

    let kw_start = markup.peek().start;
    let kind = if markup.id("for") {
        Some(RenderVariableKind::For)
    } else if markup.id("with") {
        Some(RenderVariableKind::With)
    } else {
        None
    };
    if let Some(kind) = kind {
        let name = markup.value_expression()?;
        variable = Some(Node::RenderVariableExpression(Box::new(RenderVariableExpression {
            id: ID0,
            position: Position::new(kw_start, name.position().end),
            kind,
            name,
        })));
    }

    let as_start = markup.peek().start;
    if markup.id("as") {
        let alias_token = markup.consume(T::Id)?;
        alias = Some(Node::RenderAliasExpression(Box::new(RenderAliasExpression {
            id: ID0,
            position: Position::new(as_start, alias_token.end),
            value: alias_token.value,
        })));
    }

    let mut args = Vec::new();
    markup.consume_optional(T::Comma);
    while markup.look(T::Id, 0) && markup.look(T::Colon, 1) {
        args.push(markup.named_argument()?);
        markup.consume_optional(T::Comma);
    }

    Ok(Node::RenderMarkup(Box::new(RenderMarkup {
        id: ID0,
        position: Position::new(snippet.position().start, markup.peek().start),
        snippet,
        variable,
        alias,
        args,
    })))
}

fn parse_section<'s>(markup: &mut MarkupParser<'s>) -> PResult<Node<'s>> {
    let name = string_only(markup.value_expression()?, "section name")?;
    let mut args = Vec::new();
    if markup.consume_optional(T::Comma).is_some() {
        args = markup.block_named_arguments()?;
    }
    Ok(Node::SectionMarkup(Box::new(SectionMarkup {
        id: ID0,
        position: Position::new(name.position().start, markup.peek().start),
        name,
        args,
    })))
}

// ---- {% liquid %} ----

fn is_continuation_line(prev_trimmed: &str, current_trimmed: &str) -> bool {
    if !prev_trimmed.ends_with(',') {
        return false;
    }
    // /^\w+\s*:/
    let b = current_trimmed.as_bytes();
    let word = b.iter().take_while(|&&c| js::is_word_byte(c)).count();
    if word == 0 {
        return false;
    }
    let rest = js::trim_start(&current_trimmed[word..]);
    rest.starts_with(':')
}

fn parse_lines(body: &str, body_start: u32) -> Vec<LiquidLine<'_>> {
    // First pass: join continuation lines. A logical line is a contiguous slice
    // of `body`, so it is tracked by its byte range.
    let mut joined: Vec<(usize, usize)> = Vec::new();
    let bytes = body.as_bytes();
    let mut line_start = 0usize;
    loop {
        let line_end = memchr::memchr(b'\n', &bytes[line_start..]).map_or(bytes.len(), |i| line_start + i);
        let raw_line = &body[line_start..line_end];
        let trimmed = js::trim_start(raw_line);
        let mut continued = false;
        if !trimmed.is_empty() {
            if let Some(prev) = joined.last_mut() {
                let prev_trimmed = js::trim_end(&body[prev.0..prev.1]);
                if is_continuation_line(prev_trimmed, trimmed) {
                    prev.1 = line_end;
                    continued = true;
                }
            }
        }
        if !continued {
            joined.push((line_start, line_end));
        }
        if line_end >= bytes.len() {
            break;
        }
        line_start = line_end + 1;
    }

    // Second pass: parse each logical line.
    let mut lines = Vec::with_capacity(joined.len());
    for (start, end) in joined {
        let raw = &body[start..end];
        let start_index = body_start + start as u32;
        let trimmed = js::trim_start(raw);
        if trimmed.is_empty() {
            continue;
        }
        let leading_ws = (raw.len() - trimmed.len()) as u32;
        let last_newline = memchr::memrchr(b'\n', raw.as_bytes());
        let line_end = match last_newline {
            None => start_index + js::trim_end(raw).len() as u32,
            Some(nl) => {
                start_index + nl as u32 + 1 + js::trim_end(&raw[nl + 1..]).len() as u32
            }
        };
        let first_newline = memchr::memchr(b'\n', trimmed.as_bytes());
        let first_line = first_newline.map_or(trimmed, |i| &trimmed[..i]);
        let name_offset = start_index + leading_ws;

        let (tag_name, markup, markup_offset);
        if first_line.starts_with('#') {
            tag_name = "#";
            let after = &trimmed[1..];
            markup = after.strip_prefix([' ', '\t']).unwrap_or(after);
            markup_offset =
                start_index + leading_ws + 1 + (trimmed.len() - 1 - markup.len()) as u32;
        } else if let Some(first_ws) = js::find_ws(first_line) {
            tag_name = &first_line[..first_ws];
            let rest = &trimmed[first_ws..];
            markup = js::trim_start(rest);
            markup_offset =
                start_index + leading_ws + first_ws as u32 + (rest.len() - markup.len()) as u32;
        } else {
            tag_name = first_line;
            markup = "";
            markup_offset = start_index + leading_ws + first_line.len() as u32;
        }
        lines.push(LiquidLine { tag_name, markup, markup_offset, name_offset, line_end });
    }
    lines
}

fn parse_liquid<'s>(
    markup: &mut MarkupParser<'s>,
    parser: &mut dyn StatementParser<'s>,
) -> PResult<Vec<Node<'s>>> {
    let ctx = markup.body_context();
    let body = js::slice(ctx.source, ctx.body_start, ctx.body_end);
    let mut line_ctx = LiquidLineContext { lines: parse_lines(body, ctx.body_start), index: 0 };
    let mut statements = Vec::with_capacity(line_ctx.lines.len());
    while line_ctx.index < line_ctx.lines.len() {
        let line = line_ctx.lines[line_ctx.index];
        line_ctx.index += 1;
        statements.push(parser.parse_liquid_statement(
            line.tag_name,
            line.markup,
            line.markup_offset,
            &mut line_ctx,
        )?);
    }
    Ok(statements)
}
