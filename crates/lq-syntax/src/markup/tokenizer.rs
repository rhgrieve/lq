//! Liquid expression tokens. Offsets are bytes, rebased by `start_offset`.

use crate::js;
use memchr::memchr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MarkupTokenType {
    Id,
    String,
    Number,
    Dot,
    DotDot,
    Pipe,
    Colon,
    Comma,
    OpenRound,
    CloseRound,
    OpenSquare,
    CloseSquare,
    Dash,
    Comparison,
    Equality,
    Logical,
    EndOfString,
}

impl MarkupTokenType {
    pub fn as_str(self) -> &'static str {
        match self {
            MarkupTokenType::Id => "Id",
            MarkupTokenType::String => "String",
            MarkupTokenType::Number => "Number",
            MarkupTokenType::Dot => "Dot",
            MarkupTokenType::DotDot => "DotDot",
            MarkupTokenType::Pipe => "Pipe",
            MarkupTokenType::Colon => "Colon",
            MarkupTokenType::Comma => "Comma",
            MarkupTokenType::OpenRound => "OpenRound",
            MarkupTokenType::CloseRound => "CloseRound",
            MarkupTokenType::OpenSquare => "OpenSquare",
            MarkupTokenType::CloseSquare => "CloseSquare",
            MarkupTokenType::Dash => "Dash",
            MarkupTokenType::Comparison => "Comparison",
            MarkupTokenType::Equality => "Equality",
            MarkupTokenType::Logical => "Logical",
            MarkupTokenType::EndOfString => "EndOfString",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkupToken<'s> {
    pub token_type: MarkupTokenType,
    pub value: &'s str,
    pub start: u32,
    pub end: u32,
}

/// `\d[\d_]*(\.\d[\d_]*)?` at `pos`, which must hold a digit. Returns the end.
fn scan_number(bytes: &[u8], pos: usize) -> usize {
    let digits = |mut i: usize| {
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
            i += 1;
        }
        i
    };
    let end = digits(pos + 1);
    if bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
        digits(end + 2)
    } else {
        end
    }
}

pub fn tokenize_markup(markup: &str, start_offset: u32) -> Vec<MarkupToken<'_>> {
    use MarkupTokenType as T;
    let bytes = markup.as_bytes();
    let mut tokens = Vec::with_capacity(8);
    let mut pos = 0usize;
    let mut last: Option<T> = None;

    macro_rules! push {
        ($ty:expr, $end:expr) => {{
            let end: usize = $end;
            let ty: T = $ty;
            tokens.push(MarkupToken {
                token_type: ty,
                value: &markup[pos..end],
                start: pos as u32 + start_offset,
                end: end as u32 + start_offset,
            });
            last = Some(ty);
            pos = end;
            continue;
        }};
    }

    while pos < bytes.len() {
        let w = js::ws_width_at(markup, pos);
        if w > 0 {
            pos += w;
            continue;
        }

        let ch = bytes[pos];
        let next = bytes.get(pos + 1).copied();

        if ch == b'\'' || ch == b'"' {
            if let Some(i) = memchr(ch, &bytes[pos + 1..]) {
                push!(T::String, pos + 1 + i + 1);
            }
        }

        if ch == b'-' && next.is_some_and(|n| n.is_ascii_digit()) {
            push!(T::Number, scan_number(bytes, pos + 1));
        }

        if ch.is_ascii_digit() {
            push!(T::Number, scan_number(bytes, pos));
        }

        if ch.is_ascii_alphabetic() || ch == b'_' {
            let mut end = pos + 1;
            while end < bytes.len() && (js::is_word_byte(bytes[end]) || bytes[end] == b'-') {
                end += 1;
            }
            if bytes.get(end) == Some(&b'?') {
                end += 1;
            }
            let value = &markup[pos..end];
            let mut ty = T::Id;
            if last != Some(T::Dot) {
                if value == "contains" {
                    ty = T::Comparison;
                } else if value == "and" || value == "or" {
                    ty = T::Logical;
                }
            }
            push!(ty, end);
        }

        if let Some(n) = next {
            match (ch, n) {
                (b'=', b'=') | (b'!', b'=') => push!(T::Equality, pos + 2),
                (b'>', b'=') | (b'<', b'=') => push!(T::Comparison, pos + 2),
                (b'.', b'.') => push!(T::DotDot, pos + 2),
                _ => {}
            }
        }

        let single = match ch {
            b'.' => Some(T::Dot),
            b'|' => Some(T::Pipe),
            b':' => Some(T::Colon),
            b',' => Some(T::Comma),
            b'(' => Some(T::OpenRound),
            b')' => Some(T::CloseRound),
            b'[' => Some(T::OpenSquare),
            b']' => Some(T::CloseSquare),
            b'-' => Some(T::Dash),
            b'>' | b'<' => Some(T::Comparison),
            b'=' => Some(T::Equality),
            _ => None,
        };
        if let Some(ty) = single {
            push!(ty, pos + 1);
        }

        // Unknown character: skip it.
        pos += js::char_width_at(markup, pos);
    }

    let end = markup.len() as u32 + start_offset;
    tokens.push(MarkupToken { token_type: T::EndOfString, value: "", start: end, end });
    tokens
}
