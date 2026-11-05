//! Raw tag bodies and Liquid-in-range parsing.

use super::factories::*;
use super::parser::DocumentParser;
use super::tokenizer::{Token, TokenType};
use super::tree_builder::{filter_children, ChildFilterMode};
use crate::ast::*;
use crate::errors::{Failure, PResult};
use crate::js;
use crate::liquid_doc::parser::parse_liquid_doc;

struct EndTagScanResult {
    tag_start: u32,
    tag_end: u32,
    ws_start: WhitespaceMarker,
    ws_end: WhitespaceMarker,
}

/// `/\{[{%]/.test(body)`.
pub(crate) fn has_liquid_open(body: &str) -> bool {
    let b = body.as_bytes();
    memchr::memchr_iter(b'{', b).any(|i| matches!(b.get(i + 1), Some(b'{') | Some(b'%')))
}

pub(crate) fn raw_markup_kind_for_tag(tag_name: &str, body_source: &str) -> RawMarkupKind {
    match tag_name {
        "javascript" => RawMarkupKind::Javascript,
        "stylesheet" | "style" => {
            if has_liquid_open(body_source) {
                RawMarkupKind::Text
            } else {
                RawMarkupKind::Css
            }
        }
        "schema" => RawMarkupKind::Json,
        _ => RawMarkupKind::Text,
    }
}

/// What follows the tag name in the end-tag regexes: `\s*(-?)%\}`. Returns
/// (ws_end marker, end offset of the tag).
fn scan_tag_close(source: &str, mut i: usize) -> Option<(WhitespaceMarker, usize)> {
    loop {
        let w = js::ws_width_at(source, i);
        if w == 0 {
            break;
        }
        i += w;
    }
    let rest = &source.as_bytes()[i..];
    if rest.starts_with(b"-%}") {
        Some(("-", i + 3))
    } else if rest.starts_with(b"%}") {
        Some(("", i + 2))
    } else {
        None
    }
}

/// `\{%(-?)\s*`: returns (ws_start marker, offset of the name) for a `{%` at `i`.
fn scan_tag_open(source: &str, i: usize) -> (WhitespaceMarker, usize) {
    let mut j = i + 2;
    let mut ws_start = "";
    if source.as_bytes().get(j) == Some(&b'-') {
        ws_start = "-";
        j += 1;
    }
    loop {
        let w = js::ws_width_at(source, j);
        if w == 0 {
            break;
        }
        j += w;
    }
    (ws_start, j)
}

/// `new RegExp('\\{%(-?)\\s*' + name + '\\s*(-?)%\\}')`, first match from `search_start`.
fn scan_for_first_end_tag(source: &str, search_start: usize, name: &str) -> Option<EndTagScanResult> {
    let finder = memchr::memmem::Finder::new(b"{%");
    for found in finder.find_iter(&source.as_bytes()[search_start..]) {
        let i = search_start + found;
        let (ws_start, name_at) = scan_tag_open(source, i);
        if !source.as_bytes()[name_at..].starts_with(name.as_bytes()) {
            continue;
        }
        if let Some((ws_end, tag_end)) = scan_tag_close(source, name_at + name.len()) {
            return Some(EndTagScanResult {
                tag_start: i as u32,
                tag_end: tag_end as u32,
                ws_start,
                ws_end,
            });
        }
    }
    None
}

/// Depth-balancing scan for `comment`/`doc`: nested opens pair with their own
/// ends, and a `raw` block's contents do not count.
fn scan_for_balanced_end_tag(
    source: &str,
    search_start: usize,
    open_name: &str,
    end_name: &str,
) -> Option<EndTagScanResult> {
    #[derive(PartialEq)]
    enum Name {
        Open,
        End,
        Raw,
        EndRaw,
    }
    let bytes = source.as_bytes();
    let mut depth = 0u32;
    let mut in_raw = false;
    let mut from = search_start;
    while let Some(found) = memchr::memmem::find(&bytes[from..], b"{%") {
        let i = from + found;
        from = i + 1;
        let (ws_start, name_at) = scan_tag_open(source, i);
        let rest = &bytes[name_at..];
        // Alternation order from the regex; none of the names is a prefix of
        // another. The `\b` always holds when the close matches, since what
        // follows the name is then whitespace, `-` or `%`.
        let (name, len) = if rest.starts_with(open_name.as_bytes()) {
            (Name::Open, open_name.len())
        } else if rest.starts_with(end_name.as_bytes()) {
            (Name::End, end_name.len())
        } else if rest.starts_with(b"raw") {
            (Name::Raw, 3)
        } else if rest.starts_with(b"endraw") {
            (Name::EndRaw, 6)
        } else {
            continue;
        };
        let Some((ws_end, tag_end)) = scan_tag_close(source, name_at + len) else { continue };
        from = tag_end;
        if in_raw {
            if name == Name::EndRaw {
                in_raw = false;
            }
            continue;
        }
        match name {
            Name::Raw => in_raw = true,
            Name::Open => depth += 1,
            Name::End => {
                if depth == 0 {
                    return Some(EndTagScanResult {
                        tag_start: i as u32,
                        tag_end: tag_end as u32,
                        ws_start,
                        ws_end,
                    });
                }
                depth -= 1;
            }
            Name::EndRaw => {}
        }
    }
    None
}

impl<'s> DocumentParser<'s> {
    // rawTag := "{%" tagName markup "%}" rawBody "{%" "end" tagName "%}"
    pub fn parse_raw_tag(
        &mut self,
        parse_liquid_in_body: bool,
        envelope: &LiquidTagEnvelope<'s>,
        close_token: &Token,
    ) -> PResult<Node<'s>> {
        let tag_name = envelope.tag_name;
        let source = self.source;

        let search_start = self.tokens[self.p].start as usize;
        let end_name = format!("end{tag_name}");
        let end_tag = if tag_name == "comment" || tag_name == "doc" {
            scan_for_balanced_end_tag(source, search_start, tag_name, &end_name)
        } else {
            scan_for_first_end_tag(source, search_start, &end_name)
        };
        let Some(end_tag) = end_tag else {
            return Err(Failure::parsing(
                format!("Attempting to end parsing before LiquidRawTag '{tag_name}' was closed"),
                envelope.block_start_position.start,
                envelope.block_start_position.end,
            ));
        };

        let body_start = close_token.end;
        let body_end = end_tag.tag_start;
        let body_string = js::slice(source, body_start, body_end);

        let body_nodes =
            self.parse_raw_tag_body(parse_liquid_in_body, tag_name, body_start, body_end)?;

        let body = make_raw_markup(
            raw_markup_kind_for_tag(tag_name, body_string),
            body_string,
            body_nodes,
            body_start,
            body_end,
        );

        self.reslice_tokens_from(end_tag.tag_end);

        Ok(make_liquid_raw_tag(
            envelope,
            body,
            Position::new(end_tag.tag_start, end_tag.tag_end),
            DelimiterWhitespace { start: end_tag.ws_start, end: end_tag.ws_end },
        ))
    }

    // liquidInRange := (text | liquidVariableOutput | liquidTag)*
    pub fn parse_liquid_in_range(&mut self, body_end: u32) -> PResult<Vec<Node<'s>>> {
        let mut nodes = Vec::new();
        let source = self.source;

        // Not restored when a nested parse fails: upstream has no `finally` here,
        // and the tolerant parser keeps going with HTML parsing off.
        let saved_parse_html = self.parse_html;
        self.parse_html = false;

        let mut i = self.p;
        while i < self.tokens.len() && self.tokens[i].start < body_end {
            let token = self.tokens[i];

            if token.token_type == TokenType::LiquidVariableOutputOpen
                || token.token_type == TokenType::LiquidTagOpen
            {
                let close_type = if token.token_type == TokenType::LiquidVariableOutputOpen {
                    TokenType::LiquidVariableOutputClose
                } else {
                    TokenType::LiquidTagClose
                };
                let close = self.tokens[i + 1..].iter().find(|t| t.token_type == close_type);
                let closes_within_body = close.is_some_and(|t| t.end <= body_end);

                if !closes_within_body {
                    nodes.push(make_text_node(
                        js::slice(source, token.start, body_end),
                        token.start,
                        body_end,
                    ));
                    while i < self.tokens.len() && self.tokens[i].start < body_end {
                        i += 1;
                    }
                    break;
                }

                self.p = i;
                nodes.push(if token.token_type == TokenType::LiquidVariableOutputOpen {
                    self.parse_liquid_variable_output()?
                } else {
                    self.parse_liquid_tag()?
                });
                i = self.p;
                continue;
            }

            nodes.push(make_text_node(
                js::slice(source, token.start, token.end),
                token.start,
                token.end,
            ));
            i += 1;
        }

        self.parse_html = saved_parse_html;
        self.p = i;
        Ok(filter_children(ChildFilterMode::Syntactic, nodes, source))
    }

    // rawBody := liquidDoc | (text | liquidVariableOutput | liquidTag)* | ""
    fn parse_raw_tag_body(
        &mut self,
        parse_liquid_in_body: bool,
        tag_name: &str,
        body_start: u32,
        body_end: u32,
    ) -> PResult<Vec<Node<'s>>> {
        let source = self.source;
        if tag_name == "doc" {
            return Ok(parse_liquid_doc(js::slice(source, body_start, body_end), body_start, source));
        }
        if parse_liquid_in_body {
            return self.parse_liquid_in_range(body_end);
        }
        if body_start >= body_end {
            return Ok(Vec::new());
        }
        let text = make_text_node(js::slice(source, body_start, body_end), body_start, body_end);
        Ok(filter_children(ChildFilterMode::Syntactic, vec![text], source))
    }
}
