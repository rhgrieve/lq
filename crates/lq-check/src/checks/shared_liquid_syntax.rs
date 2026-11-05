//! `checks/liquid-syntax-error/utils.ts`. Offsets are bytes; the only UTF-16 values
//! are the columns of a `ParseError`'s `loc`, which `get_offset` converts.
//!
//! The predicates run on every tag and output, so the common answers come from
//! byte scans that build no tokens. Each scan's comment says why it agrees with
//! the tokenizer; anything it can't decide goes through `tokenize_markup`.

use lq_syntax::{
    tokenize, tokenize_markup, LineCol, MarkupParser, MarkupToken, MarkupTokenType as T, Node,
    ParseError, Token, TokenType, TokenizeOptions,
};

/// `isWhitespace`. Narrower than `\s`: other JS whitespace is skipped by the
/// tokenizer without being claimed, so it counts as an uncovered character.
#[inline]
fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\n' | b'\r' | b'\t')
}

fn trim_end_ws(s: &str) -> &[u8] {
    let b = s.as_bytes();
    let mut end = b.len();
    while end > 0 && is_ws(b[end - 1]) {
        end -= 1;
    }
    &b[..end]
}

/// `resolveErrorLocation`.
pub fn resolve_error_location(error: &ParseError, source: &str) -> (u32, u32) {
    match &error.loc {
        Some(loc) => (get_offset(source, loc.start), get_offset(source, loc.end)),
        None => (0, source.len() as u32),
    }
}

/// `getOffset`. A column landing between the surrogates of a non-BMP char gives a
/// byte offset inside that char, which the engine reports as the same UTF-16 index.
pub fn get_offset(source: &str, at: LineCol) -> u32 {
    let bytes = source.as_bytes();
    let mut current_line = 1;
    let mut line_start = 0;
    while current_line != at.line {
        match bytes[line_start..].iter().position(|&b| b == b'\n') {
            Some(i) => {
                line_start += i + 1;
                current_line += 1;
            }
            None => return bytes.len() as u32,
        }
    }
    // The loop's `i < source.length` also bounds a line that starts at the end.
    if line_start >= bytes.len() {
        return bytes.len() as u32;
    }
    let mut units = at.column.saturating_sub(1);
    let mut chars = source[line_start..].char_indices();
    loop {
        if units == 0 {
            return match chars.next() {
                Some((i, _)) => (line_start + i) as u32,
                None => bytes.len() as u32,
            };
        }
        let Some((i, c)) = chars.next() else {
            return bytes.len() as u32 + units;
        };
        let w = c.len_utf16() as u32;
        if w > units {
            return (line_start + i) as u32 + 2;
        }
        units -= w;
    }
}

/// `hasBareArrayAccess`.
#[inline]
pub fn has_bare_array_access(expr: &Node<'_>) -> bool {
    matches!(expr, Node::VariableLookup(l) if l.name.is_none())
}

/// `argHasBareArrayAccess`.
pub fn arg_has_bare_array_access(arg: &Node<'_>) -> bool {
    match arg {
        Node::NamedArgument(a) => has_bare_array_access(&a.value),
        _ => has_bare_array_access(arg),
    }
}

/// `variableHasBareArrayAccess`.
pub fn variable_has_bare_array_access(variable: &Node<'_>) -> bool {
    let Node::LiquidVariable(v) = variable else { return false };
    has_bare_array_access(&v.expression)
        || v.filters.iter().any(|f| match f {
            Node::LiquidFilter(f) => f.args.iter().any(arg_has_bare_array_access),
            _ => false,
        })
}

/// `conditionalHasBareArrayAccess`.
pub fn conditional_has_bare_array_access(expr: &Node<'_>) -> bool {
    match expr {
        Node::LogicalExpression(l) => {
            conditional_has_bare_array_access(&l.left) || conditional_has_bare_array_access(&l.right)
        }
        Node::Comparison(c) => has_bare_array_access(&c.left) || has_bare_array_access(&c.right),
        _ => has_bare_array_access(expr),
    }
}

/// The byte after the string token opened by the quote at `pos`, or `None` when
/// the quote is unmatched. A quote can only be reached at a token boundary: no
/// other token type contains one, and the tokenizer takes a string whenever the
/// closing quote exists.
#[inline]
fn string_end(bytes: &[u8], pos: usize) -> Option<usize> {
    let quote = bytes[pos];
    bytes[pos + 1..].iter().position(|&b| b == quote).map(|i| pos + i + 2)
}

/// Bytes that always end up inside some token, whichever token that is.
#[inline]
fn always_claimed(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(b, b'_' | b'-' | b'.' | b'|' | b':' | b',' | b'(' | b')' | b'[' | b']' | b'=' | b'<' | b'>')
}

/// `uncoveredCharacters(markup, allowPrefixQuotes).length > 0`.
fn has_uncovered(markup: &str, allow_prefix_quotes: bool) -> bool {
    let bytes = markup.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if is_ws(b) || always_claimed(b) {
            i += 1;
        } else if b == b'\'' || b == b'"' {
            match string_end(bytes, i) {
                Some(end) => i = end,
                None if allow_prefix_quotes => i += 1,
                None => return true,
            }
        } else {
            return has_uncovered_slow(markup, allow_prefix_quotes);
        }
    }
    false
}

/// `?`, `!`, non-ASCII and anything else whose fate depends on its neighbours.
fn has_uncovered_slow(markup: &str, allow_prefix_quotes: bool) -> bool {
    let bytes = markup.as_bytes();
    let mut i = 0;
    for token in tokenize_markup(markup, 0) {
        let gap = &bytes[i..token.start as usize];
        if gap.iter().any(|&b| !is_ws(b) && !(allow_prefix_quotes && (b == b'\'' || b == b'"'))) {
            return true;
        }
        i = token.end as usize;
    }
    false
}

/// `hasSkippedCharacters`.
#[inline]
pub fn has_skipped_characters(markup: &str) -> bool {
    has_uncovered(markup, false)
}

/// `hasEmptyMarkup`: any byte that isn't whitespace is a token or uncovered.
#[inline]
pub fn has_empty_markup(markup: &str) -> bool {
    markup.bytes().all(is_ws)
}

fn meaningful_tokens(markup: &str) -> Vec<MarkupToken<'_>> {
    let mut tokens = tokenize_markup(markup, 0);
    tokens.pop();
    tokens
}

/// `hasSingleStringMarkup`.
pub fn has_single_string_markup(markup: &str, value: &str) -> bool {
    if has_skipped_characters(markup) {
        return false;
    }
    let tokens = meaningful_tokens(markup);
    let [token] = tokens.as_slice() else { return false };
    token.token_type == T::String && token.value.get(1..token.value.len() - 1) == Some(value)
}

/// `hasRubyValidInlineCommentMarkup`.
pub fn has_ruby_valid_inline_comment_markup(markup: &str) -> bool {
    let bytes = markup.as_bytes();
    if !bytes.contains(&b'\n') {
        return true;
    }
    let tokens = meaningful_tokens(markup);
    let mut next_token = 0;
    let mut continuation_lines = 0;
    let mut prefixed_continuation_lines = 0;
    let mut line = 0;
    let mut first_line_has_event = false;
    let mut line_has_event = false;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            line += 1;
            line_has_event = false;
            continue;
        }
        if is_ws(b) || line_has_event {
            continue;
        }
        line_has_event = true;
        if line == 0 {
            first_line_has_event = true;
            continue;
        }
        continuation_lines += 1;
        while next_token < tokens.len() && (tokens[next_token].end as usize) <= i {
            next_token += 1;
        }
        let covered = tokens.get(next_token).is_some_and(|t| t.start as usize <= i);
        if !covered && b == b'#' {
            prefixed_continuation_lines += 1;
        }
    }
    if continuation_lines == 0 || (!first_line_has_event && continuation_lines == 1) {
        return true;
    }
    continuation_lines == prefixed_continuation_lines
}

fn parser_for_tokens<'s>(tokens: &[MarkupToken<'s>], source: &'s str) -> MarkupParser<'s> {
    let mut all = Vec::with_capacity(tokens.len() + 1);
    all.extend_from_slice(tokens);
    let end = source.len() as u32;
    all.push(MarkupToken { token_type: T::EndOfString, value: "", start: end, end });
    MarkupParser::new(all, source, None, None)
}

fn parses_complete_value_expression(tokens: &[MarkupToken<'_>], source: &str) -> bool {
    let mut parser = parser_for_tokens(tokens, source);
    parser.value_expression().is_ok() && parser.is_at_end()
}

fn parses_complete_liquid_variable(tokens: &[MarkupToken<'_>], source: &str) -> bool {
    let mut parser = parser_for_tokens(tokens, source);
    parser.liquid_variable().is_ok() && parser.is_at_end()
}

fn parses_complete_assign_with_liquid_variable(tokens: &[MarkupToken<'_>], source: &str) -> bool {
    if tokens.len() < 3
        || tokens[0].token_type != T::Id
        || tokens[1].token_type != T::Equality
        || tokens[1].value != "="
    {
        return false;
    }
    parses_complete_liquid_variable(&tokens[2..], source)
}

/// `tokensBeforeSingleTerminalComma`.
fn tokens_before_single_terminal_comma(markup: &str) -> Option<Vec<MarkupToken<'_>>> {
    if trim_end_ws(markup).last() != Some(&b',') || has_skipped_characters(markup) {
        return None;
    }
    let mut tokens = meaningful_tokens(markup);
    let n = tokens.len();
    if n < 2 || tokens[n - 1].token_type != T::Comma || tokens[n - 2].token_type == T::Comma {
        return None;
    }
    tokens.pop();
    Some(tokens)
}

/// `hasRubyAcceptedCycleTrailingComma`.
pub fn has_ruby_accepted_cycle_trailing_comma(markup: &str) -> bool {
    tokens_before_single_terminal_comma(markup)
        .is_some_and(|tokens| parses_complete_value_expression(&tokens, markup))
}

/// `hasRubyAcceptedLoopTrailingComma`.
pub fn has_ruby_accepted_loop_trailing_comma(markup: &str) -> bool {
    let Some(tokens) = tokens_before_single_terminal_comma(markup) else { return false };
    let mut parser = parser_for_tokens(&tokens, markup);
    if parser.consume(T::Id).is_err() || !parser.id("in") || parser.value_expression().is_err() {
        return false;
    }
    parser.id("reversed");
    while !parser.is_at_end() {
        parser.consume_optional(T::Comma);
        if parser.named_argument().is_err() {
            return false;
        }
    }
    true
}

/// `hasRubyAcceptedPaginateTrailingComma`.
pub fn has_ruby_accepted_paginate_trailing_comma(markup: &str) -> bool {
    let Some(tokens) = tokens_before_single_terminal_comma(markup) else { return false };
    let mut parser = parser_for_tokens(&tokens, markup);
    if parser.value_expression().is_err() || !parser.id("by") || parser.value_expression().is_err() {
        return false;
    }
    if parser.consume_optional(T::Comma).is_some() && parser.named_arguments().is_err() {
        return false;
    }
    parser.is_at_end()
}

fn empty_first_filter_argument_with_tokens(tokens: &[MarkupToken<'_>], markup: &str) -> bool {
    let n = tokens.len();
    if n < 3
        || tokens[n - 3].token_type != T::Pipe
        || tokens[n - 2].token_type != T::Id
        || tokens[n - 1].token_type != T::Colon
    {
        return false;
    }
    let prefix = &tokens[..n - 3];
    parses_complete_liquid_variable(prefix, markup)
        || parses_complete_assign_with_liquid_variable(prefix, markup)
}

/// `hasRubyAcceptedEmptyFirstFilterArgument`. It needs a final colon token, so the
/// last byte that isn't whitespace must be a colon, or it is uncovered and fails.
pub fn has_ruby_accepted_empty_first_filter_argument(markup: &str) -> bool {
    if trim_end_ws(markup).last() != Some(&b':') || has_skipped_characters(markup) {
        return false;
    }
    empty_first_filter_argument_with_tokens(&meaningful_tokens(markup), markup)
}

/// Whether a comma could be followed by a pipe or the end. Other bytes between
/// them would be a token, or uncovered and so a rejection.
fn has_comma_before_pipe_or_end(markup: &str) -> bool {
    let bytes = markup.as_bytes();
    bytes.iter().enumerate().any(|(i, &b)| {
        b == b','
            && match bytes[i + 1..].iter().find(|&&n| !is_ws(n)) {
                None => true,
                Some(&n) => n == b'|',
            }
    })
}

/// `hasRubyAcceptedFilterArgumentTrailingComma`.
pub fn has_ruby_accepted_filter_argument_trailing_comma(markup: &str) -> bool {
    if !has_comma_before_pipe_or_end(markup) || has_skipped_characters(markup) {
        return false;
    }
    let Some(tokens) = remove_filter_argument_trailing_commas(&meaningful_tokens(markup)) else {
        return false;
    };
    parses_complete_liquid_variable(&tokens, markup)
        || parses_complete_assign_with_liquid_variable(&tokens, markup)
        || empty_first_filter_argument_with_tokens(&tokens, markup)
}

fn remove_filter_argument_trailing_commas<'s>(
    tokens: &[MarkupToken<'s>],
) -> Option<Vec<MarkupToken<'s>>> {
    let mut changed = false;
    let mut result: Vec<MarkupToken<'s>> = Vec::with_capacity(tokens.len());
    for (index, token) in tokens.iter().enumerate() {
        let next = tokens.get(index + 1);
        if token.token_type == T::Comma && next.is_none_or(|n| n.token_type == T::Pipe) {
            let after_pipe = result.iter().rposition(|t| t.token_type == T::Pipe).map_or(0, |i| i + 1);
            if !result[after_pipe..].iter().any(|t| t.token_type == T::Colon) {
                return None;
            }
            match result.last() {
                None => return None,
                Some(p) if matches!(p.token_type, T::Comma | T::Colon | T::Pipe) => return None,
                Some(_) => {}
            }
            changed = true;
            continue;
        }
        result.push(*token);
    }
    changed.then_some(result)
}

/// `hasRubyAcceptedEmptyAssignRhs`: the last token is `=`, so the last byte that
/// isn't whitespace is too.
pub fn has_ruby_accepted_empty_assign_rhs(markup: &str) -> bool {
    if trim_end_ws(markup).last() != Some(&b'=') || has_skipped_characters(markup) {
        return false;
    }
    let tokens = meaningful_tokens(markup);
    tokens.len() == 2
        && tokens[0].token_type == T::Id
        && tokens[1].token_type == T::Equality
        && tokens[1].value == "="
}

/// `hasRubyAcceptedAssignLhsExtraIdentifier`. Word bytes and dashes before the
/// first `=` can't tokenize to two identifiers: an `Id` takes them all, and a
/// leading digit or dash means the first token isn't an `Id`.
pub fn has_ruby_accepted_assign_lhs_extra_identifier(markup: &str) -> bool {
    let bytes = markup.as_bytes();
    let Some(eq) = bytes.iter().position(|&b| b == b'=') else { return false };
    let mut lhs = &bytes[..eq];
    while let [first, rest @ ..] = lhs {
        if !is_ws(*first) {
            break;
        }
        lhs = rest;
    }
    while let [rest @ .., last] = lhs {
        if !is_ws(*last) {
            break;
        }
        lhs = rest;
    }
    if lhs.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return false;
    }
    if has_skipped_characters(markup) {
        return false;
    }
    let tokens = meaningful_tokens(markup);
    if tokens.len() < 4
        || tokens[0].token_type != T::Id
        || tokens[1].token_type != T::Id
        || tokens[2].token_type != T::Equality
        || tokens[2].value != "="
    {
        return false;
    }
    parses_complete_value_expression(&tokens[3..], markup)
}

/// A value expression that parses and isn't a bare bracket lookup.
fn accepted_include_value(parser: &mut MarkupParser<'_>) -> bool {
    parser.value_expression().is_ok_and(|v| !has_bare_array_access(&v))
}

/// `hasRubyAcceptedIncludeMarkup`.
pub fn has_ruby_accepted_include_markup(markup: &str) -> bool {
    if has_skipped_characters(markup) {
        return false;
    }
    let mut parser = MarkupParser::new(tokenize_markup(markup, 0), markup, None, None);
    if !accepted_include_value(&mut parser) {
        return false;
    }
    if (parser.id("for") || parser.id("with")) && !accepted_include_value(&mut parser) {
        return false;
    }
    if parser.id("as") && parser.consume(T::Id).is_err() {
        return false;
    }
    parser.consume_optional(T::Comma);
    while parser.look(T::Id, 0) {
        if parser.consume(T::Id).is_err()
            || parser.consume(T::Colon).is_err()
            || !accepted_include_value(&mut parser)
        {
            return false;
        }
        if parser.consume_optional(T::Comma).is_none() {
            break;
        }
    }
    parser.is_at_end()
}

/// `LiquidTagBody`, with `body` borrowed from the tokenized string.
pub struct LiquidTagBody<'s> {
    pub start: u32,
    pub body: &'s str,
    pub body_start: u32,
    pub end: u32,
}

/// `liquidTagBodies`.
pub fn liquid_tag_bodies(source: &str, start_index: u32) -> Vec<LiquidTagBody<'_>> {
    liquid_tag_bodies_in(source, &tokenize(source, TokenizeOptions::default()), start_index)
}

/// `liquidTagBodies` over tokens that are already at hand.
pub fn liquid_tag_bodies_in<'s>(
    source: &'s str,
    tokens: &[Token],
    start_index: u32,
) -> Vec<LiquidTagBody<'s>> {
    let mut tags = Vec::new();
    for (i, open) in tokens.iter().enumerate() {
        if open.token_type != TokenType::LiquidTagOpen || open.start < start_index {
            continue;
        }
        let text = tokens.get(i + 1);
        let text = text.filter(|t| t.token_type == TokenType::Text);
        let close = tokens.get(i + 1 + usize::from(text.is_some()));
        let Some(close) = close.filter(|c| c.token_type == TokenType::LiquidTagClose) else {
            continue;
        };
        let (body, body_start) = match text {
            Some(t) => (&source[t.start as usize..t.end as usize], t.start),
            None => ("", open.end),
        };
        tags.push(LiquidTagBody { start: open.start, body, body_start, end: close.end });
    }
    tags
}

/// `LiquidTagMarkup`, keeping only whether `remainingTokens` is empty.
pub struct LiquidTagMarkup<'s> {
    pub tag_name: &'s str,
    pub has_remaining_tokens: bool,
    pub has_skipped_characters: bool,
}

/// `liquidTagMarkup`.
pub fn liquid_tag_markup(body: &str) -> Option<LiquidTagMarkup<'_>> {
    let tokens = meaningful_tokens(body);
    let tag_name = tokens.first()?;
    if tag_name.token_type != T::Id {
        return None;
    }
    Some(LiquidTagMarkup {
        tag_name: tag_name.value,
        has_remaining_tokens: tokens.len() > 1,
        has_skipped_characters: has_skipped_characters(body),
    })
}

/// `tagName === 'end' + name`.
pub fn is_end_tag(tag_name: &str, name: &str) -> bool {
    tag_name.strip_prefix("end") == Some(name)
}

/// `hasLiquidTagNamed`.
pub fn has_liquid_tag_named(source: &str, tag_name: &str) -> bool {
    liquid_tag_bodies(source, 0).iter().any(|tag| {
        liquid_tag_markup(tag.body).is_some_and(|m| m.tag_name == tag_name && !m.has_skipped_characters)
    })
}

/// `hasRubyAcceptedRawTagCloserWithMarkup`.
pub fn has_ruby_accepted_raw_tag_closer_with_markup(source: &str, tag_name: &str, start_index: u32) -> bool {
    for tag in liquid_tag_bodies(source, start_index) {
        let Some(markup) = liquid_tag_markup(tag.body) else { continue };
        if is_end_tag(markup.tag_name, tag_name)
            && markup.has_remaining_tokens
            && !markup.has_skipped_characters
        {
            return has_balanced_raw_tag_remainder(source, tag_name, tag.end);
        }
    }
    false
}

fn has_balanced_raw_tag_remainder(source: &str, tag_name: &str, start_index: u32) -> bool {
    let mut depth = 0;
    for tag in liquid_tag_bodies(source, start_index) {
        let Some(markup) = liquid_tag_markup(tag.body) else { continue };
        if markup.has_skipped_characters {
            continue;
        }
        if markup.tag_name == tag_name {
            depth += 1;
        } else if is_end_tag(markup.tag_name, tag_name) {
            depth -= 1;
            if depth < 0 {
                return false;
            }
        }
    }
    depth == 0
}

fn is_id_token(token: Option<&MarkupToken<'_>>, value: &str) -> bool {
    token.is_some_and(|t| t.token_type == T::Id && t.value == value)
}

/// `liquidLineTagLocation`.
pub fn liquid_line_tag_location(source: &str, tag_name: &str) -> Option<(u32, u32)> {
    for tag in liquid_tag_bodies(source, 0) {
        let Some(first_line_end) = tag.body.find('\n') else { continue };
        let first = tokenize_markup(&tag.body[..first_line_end], tag.body_start);
        if !is_id_token(first.first(), "liquid") {
            continue;
        }
        let mut line_start = tag.body_start;
        for line in tag.body.split('\n') {
            let tokens = tokenize_markup(line, line_start);
            if is_id_token(tokens.first(), tag_name) {
                return Some((tokens[0].start, line_start + line.len() as u32));
            }
            line_start += line.len() as u32 + 1;
        }
    }
    None
}

/// The statement of `tag_name` left unclosed in the `{% liquid %}` tags of `source`, which
/// starts at `offset`: the innermost, since `end<tag_name>` lines close the latest first.
/// Unlike [`liquid_line_tag_location`], which finds the first such line whether it's closed
/// or not. The end excludes trailing whitespace.
pub fn unclosed_liquid_statement(source: &str, offset: u32, tag_name: &str) -> Option<(u32, u32)> {
    let end_name = format!("end{tag_name}");
    for tag in liquid_tag_bodies(source, offset) {
        let Some(first_line_end) = tag.body.find('\n') else { continue };
        let first = tokenize_markup(&tag.body[..first_line_end], tag.body_start);
        if !is_id_token(first.first(), "liquid") {
            continue;
        }
        let mut open = Vec::new();
        let mut line_start = tag.body_start;
        for line in tag.body.split('\n') {
            let tokens = tokenize_markup(line, line_start);
            if is_id_token(tokens.first(), tag_name) {
                let end = line_start + lq_syntax::js::trim_end(line).len() as u32;
                open.push((tokens[0].start, end));
            } else if is_id_token(tokens.first(), &end_name) {
                open.pop();
            }
            line_start += line.len() as u32 + 1;
        }
        if let Some(location) = open.pop() {
            return Some(location);
        }
    }
    None
}

/// `hasSkippedPrefixCharacters`.
pub fn has_skipped_prefix_characters(source: &str, from: u32, to: u32) -> bool {
    has_uncovered(lq_syntax::js::slice(source, from, to), true)
}

/// `hasUnclosedQuotedString`.
pub fn has_unclosed_quoted_string(markup: &str) -> bool {
    let bytes = markup.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' || bytes[i] == b'"' {
            match string_end(bytes, i) {
                Some(end) => i = end,
                None => return true,
            }
        } else {
            i += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Upstream tests this helper directly: the parser's `{% liquid %}` failures
    /// surface as a `reason`, so `check()` doesn't reach it with these inputs.
    #[test]
    fn locates_line_mode_tags() {
        assert_eq!(liquid_line_tag_location("{% liquid\n  block 'hero'\n%}", "block"), Some((12, 24)));
        assert_eq!(liquid_line_tag_location("{% if a %}", "block"), None);
        assert_eq!(unclosed_liquid_statement("{% liquid\n  block 'hero'\n%}", 0, "block"), Some((12, 24)));
        assert_eq!(unclosed_liquid_statement("{% liquid\n block 'a'\n endblock\n block 'b'\n%}", 0, "block"), Some((32, 41)));
        assert_eq!(unclosed_liquid_statement("{% liquid\n block 'a'\n endblock\n%}", 0, "block"), None);
    }

    #[test]
    fn get_offset_converts_utf16_columns() {
        let at = |line, column| LineCol { line, column };
        assert_eq!(get_offset("", at(1, 1)), 0);
        assert_eq!(get_offset("ab\ncd", at(2, 2)), 4);
        assert_eq!(get_offset("\u{1F600}a", at(1, 3)), 4);
        // Between the surrogates.
        assert_eq!(get_offset("\u{1F600}", at(1, 2)), 2);
        assert_eq!(get_offset("a\n", at(2, 1)), 2);
    }
}
