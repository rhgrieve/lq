//! `MarkupParser`: the Liquid expression grammar, including the lax (Ruby render
//! parity) and tolerant recovery axes. Lax code is reachable only through
//! `enable_lax`, which only `lax_recover` uses.

use super::expression_adapter::{adapt_complex, adapt_conditional, Expr};
use super::tokenizer::{tokenize_markup, MarkupToken, MarkupTokenType as T};
use crate::ast::*;
use crate::errors::{Failure, PResult};
use crate::js;
use std::borrow::Cow;

const ID0: NodeId = NodeId(0);

/// The raw markup range from `MarkupParser::body_context`.
#[derive(Clone, Copy, Debug)]
pub struct BodyContext<'s> {
    pub source: &'s str,
    pub body_start: u32,
    pub body_end: u32,
}

#[derive(Clone, Debug)]
pub struct MarkupParser<'s> {
    tokens: Vec<MarkupToken<'s>>,
    source: &'s str,
    p: usize,
    markup_start: u32,
    markup_end: u32,
    lax: bool,
    tolerant: bool,
    in_condition: bool,
}

struct Run<'s> {
    value: &'s str,
    start: u32,
    end: u32,
}

fn is_comparison_op(s: &str) -> bool {
    matches!(s, "==" | "!=" | "<" | ">" | "<=" | ">=" | "contains")
}

fn is_symbolic_operator(tok: &MarkupToken<'_>) -> bool {
    matches!(tok.token_type, T::Equality | T::Comparison)
        && !tok.value.is_empty()
        && tok.value.bytes().all(|b| matches!(b, b'=' | b'<' | b'>' | b'!'))
}

/// `s in LiquidLiteralValues`, which includes `Object.prototype`'s members.
fn literal_value(s: &str) -> Option<LiteralValue> {
    Some(match s {
        "nil" | "null" => LiteralValue::Null,
        "true" => LiteralValue::True,
        "false" => LiteralValue::False,
        "blank" | "empty" => LiteralValue::EmptyString,
        "__proto__" => LiteralValue::ObjectPrototype,
        "constructor" | "__defineGetter__" | "__defineSetter__" | "hasOwnProperty"
        | "__lookupGetter__" | "__lookupSetter__" | "isPrototypeOf" | "propertyIsEnumerable"
        | "toString" | "valueOf" | "toLocaleString" => LiteralValue::ObjectPrototypeFunction,
        _ => return None,
    })
}

fn extend_logical_end(node: &mut Node<'_>, end: u32) {
    if let Node::LogicalExpression(l) = node {
        l.position.end = end;
        extend_logical_end(&mut l.right, end);
    }
}

impl<'s> MarkupParser<'s> {
    pub fn new(
        tokens: Vec<MarkupToken<'s>>,
        source: &'s str,
        markup_start: Option<u32>,
        markup_end: Option<u32>,
    ) -> Self {
        let first_start = tokens.first().map_or(0, |t| t.start);
        let markup_end = markup_end.unwrap_or_else(|| {
            tokens
                .iter()
                .rev()
                .find(|t| t.token_type != T::EndOfString)
                .map_or(first_start, |t| t.end)
        });
        MarkupParser {
            tokens,
            source,
            p: 0,
            markup_start: markup_start.unwrap_or(first_start),
            markup_end,
            lax: false,
            tolerant: false,
            in_condition: false,
        }
    }

    pub fn enable_lax(&mut self) -> &mut Self {
        self.lax = true;
        self
    }

    pub fn is_lax(&self) -> bool {
        self.lax
    }

    pub fn enable_tolerant(&mut self) -> &mut Self {
        self.tolerant = true;
        self
    }

    pub fn is_tolerant(&self) -> bool {
        self.tolerant
    }

    pub fn source(&self) -> &'s str {
        self.source
    }

    /// Raw source from the current token up to (excluding) a token of type
    /// `stop` or end of markup, trimmed; advances past what it covers.
    pub fn consume_raw_until(&mut self, stop: T) -> &'s str {
        let start = self.peek().start;
        let mut end = start;
        while !self.is_at_end() && self.peek().token_type != stop {
            end = self.peek().end;
            self.p += 1;
        }
        js::trim(js::slice(self.source, start, end))
    }

    fn discard_remaining(&mut self) {
        while !self.is_at_end() {
            self.p += 1;
        }
    }

    /// No-op unless lax.
    pub fn discard_trailing(&mut self) {
        if self.lax {
            self.discard_remaining();
        }
    }

    pub fn at_value_start(&self) -> bool {
        match self.peek().token_type {
            T::String | T::Number | T::Id | T::OpenSquare | T::OpenRound => true,
            T::Dash => self.lax,
            _ => false,
        }
    }

    fn at_argument_start(&self) -> bool {
        matches!(self.peek().token_type, T::String | T::Number | T::Id | T::OpenSquare | T::OpenRound)
    }

    fn open_square_continues_lookup(&self, prev_end: u32, open_square: &MarkupToken<'s>) -> bool {
        if !self.lax {
            return true;
        }
        !js::has_non_ws(js::slice(self.source, prev_end, open_square.start))
    }

    fn skip_to_next_pipe(&mut self) {
        while !self.is_at_end() && !self.look(T::Pipe, 0) {
            self.p += 1;
        }
    }

    pub fn consume(&mut self, token_type: T) -> PResult<MarkupToken<'s>> {
        let token = self.tokens[self.p];
        if token.token_type != token_type {
            return Err(Failure::parsing(
                format!(
                    "Expected {} but found {} ({})",
                    token_type.as_str(),
                    token.token_type.as_str(),
                    token.value
                ),
                token.start,
                token.end,
            ));
        }
        self.p += 1;
        Ok(token)
    }

    pub fn consume_optional(&mut self, token_type: T) -> Option<MarkupToken<'s>> {
        let token = self.tokens[self.p];
        if token.token_type != token_type {
            return None;
        }
        self.p += 1;
        Some(token)
    }

    pub fn look(&self, token_type: T, ahead: usize) -> bool {
        self.tokens.get(self.p + ahead).is_some_and(|t| t.token_type == token_type)
    }

    #[inline]
    pub fn peek(&self) -> MarkupToken<'s> {
        self.tokens[self.p]
    }

    /// Consumes the `Id` token `keyword` if it is next.
    pub fn id(&mut self, keyword: &str) -> bool {
        let token = &self.tokens[self.p];
        if token.token_type != T::Id || token.value != keyword {
            return false;
        }
        self.p += 1;
        true
    }

    #[inline]
    pub fn is_at_end(&self) -> bool {
        self.tokens[self.p].token_type == T::EndOfString
    }

    pub fn eos_start(&self) -> u32 {
        self.tokens[self.tokens.len() - 1].start
    }

    /// The full raw markup range; advances to end of string.
    pub fn body_context(&mut self) -> BodyContext<'s> {
        self.discard_remaining();
        BodyContext { source: self.source, body_start: self.markup_start, body_end: self.markup_end }
    }

    /// Source from the current token to the end of markup, trimmed.
    pub fn remaining_source(&self) -> &'s str {
        let current = self.tokens[self.p];
        if current.token_type == T::EndOfString {
            return "";
        }
        js::trim(js::slice(self.source, current.start, self.eos_start()))
    }

    fn expected_expression(token: &MarkupToken<'s>) -> Failure {
        Failure::parsing(
            format!(
                "Expected expression but found {} ({})",
                token.token_type.as_str(),
                token.value
            ),
            token.start,
            token.end,
        )
    }

    fn bare_lookup(&self, name: Option<&'s str>, start: u32, end: u32) -> Node<'s> {
        Node::VariableLookup(Box::new(LiquidVariableLookup {
            id: ID0,
            position: Position::new(start, end),
            name,
            lookups: Vec::new(),
            lookup_modes: None,
        }))
    }

    // valueExpression := string | number | literal | variableLookup | range
    pub fn value_expression(&mut self) -> PResult<Node<'s>> {
        let token = self.peek();
        match token.token_type {
            T::String => {
                self.p += 1;
                let v = token.value;
                Ok(Node::String(Box::new(LiquidString {
                    id: ID0,
                    position: Position::new(token.start, token.end),
                    single: Some(v.as_bytes()[0] == b'\''),
                    value: &v[1..v.len() - 1],
                })))
            }
            T::Number => {
                if self.lax {
                    if let Some(next) = self.tokens.get(self.p + 1).copied() {
                        if next.token_type == T::Id && next.start == token.end {
                            self.p += 2;
                            let name = js::slice(self.source, token.start, next.end);
                            return Ok(self.bare_lookup(Some(name), token.start, next.end));
                        }
                    }
                    if self.lax_lookup_separator_follows(token.end) {
                        return self.variable_lookup();
                    }
                }
                self.p += 1;
                Ok(Node::Number(Box::new(LiquidNumber {
                    id: ID0,
                    position: Position::new(token.start, token.end),
                    value: token.value,
                })))
            }
            T::Id => {
                let followed_by_bracket_lookup = match self.tokens.get(self.p + 1) {
                    Some(next) if next.token_type == T::OpenSquare => {
                        self.open_square_continues_lookup(token.end, next)
                    }
                    _ => false,
                };
                if let Some(value) = literal_value(token.value) {
                    if !self.look(T::Dot, 1)
                        && !followed_by_bracket_lookup
                        && !(self.lax && self.lax_lookup_separator_follows(token.end))
                    {
                        self.p += 1;
                        return Ok(Node::LiquidLiteral(Box::new(LiquidLiteral {
                            id: ID0,
                            position: Position::new(token.start, token.end),
                            keyword: token.value,
                            value,
                        })));
                    }
                }
                self.variable_lookup()
            }
            T::OpenSquare => self.variable_lookup(),
            T::Dot => {
                if self.lax {
                    if let Some(next) = self.tokens.get(self.p + 1).copied() {
                        if next.token_type == T::Number && next.start == token.end {
                            let has_dot = next.value.contains('.');
                            let mut seg_end = if has_dot {
                                next.start + next.value.find('.').unwrap_or(0) as u32
                            } else {
                                next.end
                            };
                            let mut end = next.end;
                            let mut consumed = 2;
                            if !has_dot {
                                while let Some(after) = self.tokens.get(self.p + consumed) {
                                    if after.start == end
                                        && !after.value.contains('.')
                                        && matches!(after.token_type, T::Id | T::Dash | T::Number)
                                    {
                                        seg_end = after.end;
                                        end = after.end;
                                        consumed += 1;
                                    } else {
                                        break;
                                    }
                                }
                            }
                            self.p += consumed;
                            let name = js::slice(self.source, next.start, seg_end);
                            return Ok(self.bare_lookup(Some(name), token.start, end));
                        }
                    }
                }
                Err(Self::expected_expression(&token))
            }
            T::OpenRound => {
                let open_token = self.consume(T::OpenRound)?;
                if self.lax && !self.paren_contains_range() {
                    let raw = self.consume_raw_until(T::CloseRound);
                    self.consume_optional(T::CloseRound);
                    self.discard_remaining();
                    return Ok(self.bare_lookup(Some(raw), open_token.start, self.eos_start()));
                }
                let start = self.value_expression()?;
                self.consume(T::DotDot)?;
                if self.lax {
                    while self.look(T::Dot, 0) || self.look(T::DotDot, 0) {
                        self.p += 1;
                    }
                }
                let end = self.value_expression()?;
                let close_token = if self.lax {
                    self.consume_optional(T::CloseRound)
                } else {
                    Some(self.consume(T::CloseRound)?)
                };
                let range_end = close_token.map_or(end.position().end, |t| t.end);
                Ok(Node::Range(Box::new(LiquidRange {
                    id: ID0,
                    position: Position::new(open_token.start, range_end),
                    start,
                    end,
                })))
            }
            T::Dash => {
                if self.lax {
                    self.p += 1;
                    return Ok(self.bare_lookup(Some("-"), token.start, token.end));
                }
                Err(Self::expected_expression(&token))
            }
            _ => Err(Self::expected_expression(&token)),
        }
    }

    fn paren_contains_range(&self) -> bool {
        let mut depth = 0;
        for t in &self.tokens[self.p..] {
            match t.token_type {
                T::EndOfString => break,
                T::OpenRound => depth += 1,
                T::CloseRound => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                T::DotDot if depth == 0 => return true,
                _ => {}
            }
        }
        false
    }

    // variableLookup := (id | "[" valueExpression "]") ("." id | "[" valueExpression "]")*
    pub fn variable_lookup(&mut self) -> PResult<Node<'s>> {
        let name: Option<&'s str>;
        let start_pos: u32;

        let current = self.peek();
        if current.token_type == T::Id {
            let token = self.consume(T::Id)?;
            name = Some(token.value);
            start_pos = token.start;
        } else if current.token_type == T::OpenSquare {
            name = None;
            start_pos = current.start;
        } else if self.lax
            && current.token_type == T::Number
            && self.lax_lookup_separator_follows(current.end)
        {
            self.p += 1;
            name = Some(current.value);
            start_pos = current.start;
        } else {
            return Err(Failure::parsing(
                format!(
                    "Expected identifier or [ but found {} ({})",
                    current.token_type.as_str(),
                    current.value
                ),
                current.start,
                current.end,
            ));
        }

        let mut lookups: Vec<Node<'s>> = Vec::new();
        let mut lookup_modes: Vec<LookupMode> = Vec::new();
        let mut end_pos = if name.is_some() { self.tokens[self.p - 1].end } else { start_pos };

        while !self.is_at_end() {
            if self.consume_optional(T::Dot).is_some() {
                let prop = self.consume(T::Id)?;
                lookups.push(Node::String(Box::new(LiquidString {
                    id: ID0,
                    position: Position::new(prop.start, prop.end),
                    single: None,
                    value: prop.value,
                })));
                lookup_modes.push(LookupMode::Bareword);
                end_pos = prop.end;
            } else if self.look(T::OpenSquare, 0)
                && self.open_square_continues_lookup(end_pos, &self.peek())
            {
                self.consume(T::OpenSquare)?;
                let inner = self.value_expression()?;
                let close = if self.lax {
                    self.consume_optional(T::CloseSquare)
                } else {
                    Some(self.consume(T::CloseSquare)?)
                };
                end_pos = close.map_or(inner.position().end, |t| t.end);
                lookups.push(inner);
                lookup_modes.push(LookupMode::Subscript);
                if self.lax && close.is_none() && !self.look(T::Dot, 0) && !self.look(T::OpenSquare, 0)
                {
                    break;
                }
            } else if self.lax && !self.in_condition {
                let Some(seg) = self.try_consume_lax_lookup_separator(end_pos) else { break };
                lookups.push(Node::String(Box::new(LiquidString {
                    id: ID0,
                    position: Position::new(seg.start, seg.end),
                    single: None,
                    value: seg.value,
                })));
                lookup_modes.push(LookupMode::Bareword);
                end_pos = seg.end;
            } else {
                break;
            }
        }

        if self.lax && name.is_none() && lookups.len() == 1 {
            if let Node::VariableLookup(inner) = &mut lookups[0] {
                if inner.name.is_none() && inner.lookups.len() == 1 {
                    let inner_mode =
                        inner.lookup_modes.as_ref().and_then(|m| m.first().copied());
                    let hoisted = inner.lookups.remove(0);
                    return Ok(Node::VariableLookup(Box::new(LiquidVariableLookup {
                        id: ID0,
                        position: Position::new(start_pos, end_pos),
                        name: None,
                        lookups: vec![hoisted],
                        lookup_modes: Some(vec![inner_mode.unwrap_or(LookupMode::Subscript)]),
                    })));
                }
            }
        }

        Ok(Node::VariableLookup(Box::new(LiquidVariableLookup {
            id: ID0,
            position: Position::new(start_pos, end_pos),
            name,
            lookups,
            lookup_modes: Some(lookup_modes),
        })))
    }

    /// `/[\w-]+\??/y` at `at`: the end of the match, if any.
    fn word_segment_end(&self, at: u32) -> Option<u32> {
        let bytes = self.source.as_bytes();
        let start = at as usize;
        let mut i = start;
        while i < bytes.len() && (js::is_word_byte(bytes[i]) || bytes[i] == b'-') {
            i += 1;
        }
        if i == start {
            return None;
        }
        if bytes.get(i) == Some(&b'?') {
            i += 1;
        }
        Some(i as u32)
    }

    fn try_consume_lax_lookup_separator(&mut self, prev_end: u32) -> Option<Run<'s>> {
        let mut i = self.p;
        let mut expected_start = prev_end;
        while i < self.tokens.len() {
            let tok = &self.tokens[i];
            if tok.token_type == T::EndOfString || tok.start != expected_start {
                return None;
            }
            if !is_symbolic_operator(tok) {
                break;
            }
            expected_start = tok.end;
            i += 1;
        }
        if i == self.p {
            return None;
        }
        let end = self.word_segment_end(expected_start)?;
        let mut j = i;
        while j < self.tokens.len()
            && self.tokens[j].token_type != T::EndOfString
            && self.tokens[j].start >= expected_start
            && self.tokens[j].end <= end
        {
            j += 1;
        }
        self.p = j;
        Some(Run { value: js::slice(self.source, expected_start, end), start: expected_start, end })
    }

    fn lax_lookup_separator_follows(&self, prev_end: u32) -> bool {
        let start_index = self.p + 1;
        let mut i = start_index;
        let mut expected_start = prev_end;
        while i < self.tokens.len() {
            let tok = &self.tokens[i];
            if tok.token_type == T::EndOfString || tok.start != expected_start {
                break;
            }
            if !is_symbolic_operator(tok) {
                break;
            }
            expected_start = tok.end;
            i += 1;
        }
        if i == start_index {
            return false;
        }
        if !self.in_condition {
            return true;
        }
        self.word_segment_end(expected_start).is_some()
    }

    fn comparison(&mut self) -> PResult<Expr<'s>> {
        if self.lax && self.in_condition {
            self.skip_lax_condition_parens();
        }
        let left = self.value_expression()?;
        let token = self.peek();

        if self.lax && self.in_condition && token.token_type != T::Logical {
            if let Some(op_run) = self.lax_condition_operator_run(left.position().end) {
                if !is_comparison_op(op_run.value) {
                    return Ok(self.unknown_operator_comparison(left, op_run));
                }
                if token.token_type != T::Comparison && token.token_type != T::Equality {
                    return self.merged_operator_comparison(left, op_run);
                }
            }
        }

        if token.token_type != T::Comparison && token.token_type != T::Equality {
            if self.lax {
                self.consume_optional(T::CloseRound);
            }
            return Ok(Expr::Value(left));
        }

        if !is_comparison_op(token.value) {
            if self.lax {
                return Ok(Expr::Value(left));
            }
            return Err(Failure::parsing(
                format!("Unknown operator {}", token.value),
                token.start,
                token.end,
            ));
        }

        self.p += 1;
        if self.lax && !self.at_value_start() {
            return Ok(Expr::Value(left));
        }
        let right = self.value_expression()?;

        if self.lax {
            self.consume_optional(T::CloseRound);
        }

        let position = Position::new(left.position().start, right.position().end);
        Ok(Expr::Comparison { left, op: token.value, right, position })
    }

    fn lax_condition_operator_run(&self, after_left: u32) -> Option<Run<'s>> {
        let mut start = after_left;
        while start < self.markup_end {
            let w = js::ws_width_at(self.source, start as usize);
            if w == 0 {
                break;
            }
            start += w as u32;
        }
        let rest = js::slice(self.source, start, self.markup_end);
        let len = rest
            .bytes()
            .take_while(|b| matches!(b, b'=' | b'!' | b'<' | b'>' | b'a'..=b'z' | b'_'))
            .count();
        if len == 0 {
            return None;
        }
        Some(Run { value: &rest[..len], start, end: start + len as u32 })
    }

    fn unknown_operator_comparison(&mut self, left: Node<'s>, op: Run<'s>) -> Expr<'s> {
        while self.peek().token_type != T::EndOfString && self.peek().start < op.end {
            self.p += 1;
        }
        let mut right = None;
        if self.at_value_start() {
            let saved_p = self.p;
            match self.value_expression() {
                Ok(r) => right = Some(r),
                Err(_) => {
                    self.p = saved_p;
                    while self.peek().token_type != T::EndOfString
                        && self.peek().token_type != T::Logical
                    {
                        self.p += 1;
                    }
                }
            }
        }
        self.consume_optional(T::CloseRound);
        let right = right.unwrap_or_else(|| left.clone());
        let position = Position::new(left.position().start, right.position().end);
        Expr::Comparison { left, op: op.value, right, position }
    }

    fn merged_operator_comparison(&mut self, left: Node<'s>, op: Run<'s>) -> PResult<Expr<'s>> {
        while self.peek().token_type != T::EndOfString && self.peek().start < op.end {
            self.p += 1;
        }
        let rest = js::slice(self.source, op.end, self.markup_end);
        let mut rest_parser = MarkupParser::new(
            tokenize_markup(rest, op.end),
            self.source,
            Some(op.end),
            Some(self.markup_end),
        );
        rest_parser.enable_lax();
        let right = if rest_parser.at_value_start() {
            rest_parser.value_expression()?
        } else {
            self.bare_lookup(None, op.end, op.end)
        };
        while self.peek().token_type != T::EndOfString && self.peek().start < right.position().end {
            self.p += 1;
        }
        self.consume_optional(T::CloseRound);
        let position = Position::new(left.position().start, right.position().end);
        Ok(Expr::Comparison { left, op: op.value, right, position })
    }

    fn skip_lax_condition_parens(&mut self) {
        while self.look(T::OpenRound, 0) && !self.paren_at_cursor_contains_range() {
            self.p += 1;
        }
    }

    fn paren_at_cursor_contains_range(&self) -> bool {
        let mut depth = 0;
        for t in &self.tokens[self.p..] {
            match t.token_type {
                T::EndOfString => break,
                T::OpenRound => depth += 1,
                T::CloseRound => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                T::DotDot if depth == 1 => return true,
                _ => {}
            }
        }
        false
    }

    fn logical_expr(&mut self) -> PResult<Expr<'s>> {
        let left = self.comparison()?;
        let token = self.peek();

        if token.token_type != T::Logical {
            return Ok(left);
        }

        // The tokenizer only makes Logical tokens of `and` and `or`.
        let relation = if token.value == "and" { LogicalRelation::And } else { LogicalRelation::Or };

        self.p += 1;
        let right = self.logical_expr()?;
        let position = Position::new(left.position().start, right.position().end);
        Ok(Expr::Logical {
            left: Box::new(left),
            relation,
            right: Box::new(right),
            op_start: token.start,
            position,
        })
    }

    pub fn conditional_expression(&mut self) -> PResult<Node<'s>> {
        let saved_in_condition = self.in_condition;
        self.in_condition = true;
        let result = self.logical_expr();
        self.in_condition = saved_in_condition;
        let mut result = adapt_conditional(result?);
        if self.lax {
            self.discard_remaining();
        }
        if matches!(result, Node::LogicalExpression(_)) {
            extend_logical_end(&mut result, self.eos_start());
        }
        Ok(result)
    }

    pub fn expression(&mut self) -> PResult<Node<'s>> {
        Ok(adapt_complex(self.logical_expr()?))
    }

    // liquidVariable := expression filter*
    pub fn liquid_variable(&mut self) -> PResult<Node<'s>> {
        let expr = self.expression()?;
        let expr_position = expr.position();
        let filter_list = self.filters(expr_position.end)?;
        if self.lax {
            self.discard_remaining();
        }
        let token_end = filter_list.last().map_or(expr_position.end, |f| f.position().end);
        let start = expr_position.start;
        Ok(Node::LiquidVariable(Box::new(LiquidVariable {
            id: ID0,
            position: Position::new(start, self.eos_start()),
            expression: expr,
            filters: filter_list,
            raw_source: js::slice(self.source, start, token_end),
        })))
    }

    pub fn filters(&mut self, mut previous_end: u32) -> PResult<Vec<Node<'s>>> {
        let mut result = Vec::new();
        while self.consume_optional(T::Pipe).is_some() {
            if (self.lax || self.tolerant) && !self.look(T::Id, 0) {
                self.skip_to_next_pipe();
                continue;
            }
            let filter_node = self.filter(previous_end)?;
            previous_end = filter_node.position().end;
            result.push(filter_node);
            if self.lax && !self.is_at_end() && !self.look(T::Pipe, 0) {
                self.skip_to_next_pipe();
            }
        }
        Ok(result)
    }

    pub fn filter(&mut self, previous_end: u32) -> PResult<Node<'s>> {
        let name_token = self.consume(T::Id)?;
        let mut args = Vec::new();
        let mut end = name_token.end;
        if let Some(colon) = self.consume_optional(T::Colon) {
            end = colon.end;
            let at_filter_boundary = self.is_at_end() || self.look(T::Pipe, 0);
            let only_ws_before_boundary =
                !js::has_non_ws(js::slice(self.source, colon.end, self.peek().start));
            let empty_arguments = at_filter_boundary && only_ws_before_boundary;
            if !empty_arguments && (!(self.lax || self.tolerant) || self.at_argument_start()) {
                args = self.arguments()?;
            }
            if let Some(last) = args.last() {
                end = last.position().end;
            }
        }
        Ok(Node::LiquidFilter(Box::new(LiquidFilter {
            id: ID0,
            position: Position::new(previous_end, end),
            name: name_token.value,
            args,
        })))
    }

    pub fn argument(&mut self) -> PResult<Node<'s>> {
        if self.look(T::Id, 0) && self.lookahead_for_named_argument() {
            return self.named_argument();
        }
        self.value_expression()
    }

    fn lookahead_for_named_argument(&self) -> bool {
        let mut ahead = 1;
        while self.look(T::Dot, ahead) && self.look(T::Id, ahead + 1) {
            ahead += 2;
        }
        self.look(T::Colon, ahead)
    }

    /// `id ("." id)*`, the name of a named argument.
    fn named_argument_name(&mut self) -> PResult<(MarkupToken<'s>, Cow<'s, str>)> {
        let name_token = self.consume(T::Id)?;
        let mut name = Cow::Borrowed(name_token.value);
        while self.consume_optional(T::Dot).is_some() {
            let segment = self.consume(T::Id)?;
            let owned = name.to_mut();
            owned.push('.');
            owned.push_str(segment.value);
        }
        Ok((name_token, name))
    }

    pub fn named_argument(&mut self) -> PResult<Node<'s>> {
        let (name_token, name) = self.named_argument_name()?;
        self.consume(T::Colon)?;
        let value = self.value_expression()?;
        Ok(Node::NamedArgument(Box::new(NamedArgument {
            id: ID0,
            position: Position::new(name_token.start, value.position().end),
            name,
            value,
        })))
    }

    pub fn arguments(&mut self) -> PResult<Vec<Node<'s>>> {
        let mut args = vec![self.argument()?];
        while self.consume_optional(T::Comma).is_some() {
            if (self.lax || self.tolerant) && !self.at_argument_start() {
                break;
            }
            args.push(self.argument()?);
        }
        Ok(args)
    }

    pub fn named_arguments(&mut self) -> PResult<Vec<Node<'s>>> {
        let mut args = vec![self.named_argument()?];
        while self.consume_optional(T::Comma).is_some() {
            if !self.look(T::Id, 0) {
                break;
            }
            args.push(self.named_argument()?);
        }
        Ok(args)
    }

    pub fn block_named_arguments(&mut self) -> PResult<Vec<Node<'s>>> {
        let mut args = vec![self.block_named_argument()?];
        while self.consume_optional(T::Comma).is_some() {
            if !self.look(T::Id, 0) {
                break;
            }
            args.push(self.block_named_argument()?);
        }
        Ok(args)
    }

    fn block_named_argument(&mut self) -> PResult<Node<'s>> {
        let (name_token, name) = self.named_argument_name()?;
        self.consume(T::Colon)?;
        let value = if self.look(T::OpenSquare, 0) {
            self.block_array_literal()?
        } else {
            self.value_expression()?
        };
        Ok(Node::NamedArgument(Box::new(NamedArgument {
            id: ID0,
            position: Position::new(name_token.start, value.position().end),
            name,
            value,
        })))
    }

    fn block_array_literal(&mut self) -> PResult<Node<'s>> {
        let open_token = self.consume(T::OpenSquare)?;
        let mut elements = Vec::new();
        while !self.look(T::CloseSquare, 0) {
            if self.look(T::OpenSquare, 0) {
                let token = self.peek();
                return Err(Failure::parsing(
                    "nested arrays are not supported in block array literal",
                    token.start,
                    token.end,
                ));
            }
            elements.push(self.value_expression()?);
            if !self.look(T::CloseSquare, 0) {
                self.consume(T::Comma)?;
            }
        }
        let close_token = self.consume(T::CloseSquare)?;
        Ok(Node::BlockArrayLiteral(Box::new(BlockArrayLiteral {
            id: ID0,
            position: Position::new(open_token.start, close_token.end),
            elements,
        })))
    }
}
