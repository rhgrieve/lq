//! Two-level LiquidDoc tokenization: the body, then a `@param` line.

use crate::js;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiquidDocTokenType {
    Annotation,
    TextLine,
    Newline,
    EndOfInput,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LiquidDocToken<'s> {
    pub token_type: LiquidDocTokenType,
    pub value: &'s str,
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParamTokenType {
    Type,
    OptionalName,
    Word,
    Dash,
    Text,
    EndOfInput,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ParamToken<'s> {
    pub token_type: ParamTokenType,
    pub value: &'s str,
    pub start: u32,
    pub end: u32,
}

/// `/[ \t]+/y`: end of the run from `pos`, which may be `pos` itself.
fn skip_blanks(bytes: &[u8], mut pos: usize) -> usize {
    while pos < bytes.len() && (bytes[pos] == b' ' || bytes[pos] == b'\t') {
        pos += 1;
    }
    pos
}

fn word_end(bytes: &[u8], mut pos: usize) -> usize {
    while pos < bytes.len() && js::is_word_byte(bytes[pos]) {
        pos += 1;
    }
    pos
}

/// `/.@\w+/` on one line: the offset of the first `@` that has a preceding char
/// other than a line terminator and is followed by a word char. `.` matches one
/// UTF-16 unit there, which only affects where the `.` starts, not the `@`.
fn midline_annotation(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    for at in memchr::memchr_iter(b'@', bytes) {
        if at == 0 || !bytes.get(at + 1).copied().is_some_and(js::is_word_byte) {
            continue;
        }
        let before = line[..at].chars().next_back();
        if matches!(before, Some('\r') | Some('\n') | Some('\u{2028}') | Some('\u{2029}')) {
            continue;
        }
        return Some(at);
    }
    None
}

pub(crate) fn tokenize_liquid_doc(body: &str, start_offset: u32) -> Vec<LiquidDocToken<'_>> {
    use LiquidDocTokenType as T;
    let bytes = body.as_bytes();
    let mut tokens = Vec::new();
    let mut pos = 0usize;
    let mut after_annotation = false;

    let abs = |p: usize| p as u32 + start_offset;

    while pos < bytes.len() {
        // /\r?\n/y
        let newline_end = if bytes[pos] == b'\n' {
            Some(pos + 1)
        } else if bytes[pos] == b'\r' && bytes.get(pos + 1) == Some(&b'\n') {
            Some(pos + 2)
        } else {
            None
        };
        if let Some(end) = newline_end {
            tokens.push(LiquidDocToken {
                token_type: T::Newline,
                value: &body[pos..end],
                start: abs(pos),
                end: abs(end),
            });
            pos = end;
            after_annotation = false;
            continue;
        }

        let after_ws = skip_blanks(bytes, pos);
        if bytes.get(after_ws) == Some(&b'@') {
            let end = word_end(bytes, after_ws + 1);
            if end > after_ws + 1 {
                tokens.push(LiquidDocToken {
                    token_type: T::Annotation,
                    value: &body[after_ws + 1..end],
                    start: abs(after_ws),
                    end: abs(end),
                });
                pos = skip_blanks(bytes, end);
                after_annotation = true;
                continue;
            }
        }

        let slice_end = memchr::memchr(b'\n', &bytes[pos..]).map_or(bytes.len(), |i| pos + i);
        let mut effective_end = slice_end;
        if !after_annotation {
            if let Some(at) = midline_annotation(&body[pos..slice_end]) {
                effective_end = pos + at;
            }
        }
        if effective_end > pos {
            tokens.push(LiquidDocToken {
                token_type: T::TextLine,
                value: &body[pos..effective_end],
                start: abs(pos),
                end: abs(effective_end),
            });
        }
        pos = effective_end;
        after_annotation = false;
    }

    let end = abs(bytes.len());
    tokens.push(LiquidDocToken { token_type: T::EndOfInput, value: "", start: end, end });
    tokens
}

/// The exclusive end of the `{type}` at `start`, if one is there.
fn read_type_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&b'{') {
        return None;
    }
    let mut pos = start + 1;
    while pos < bytes.len() {
        let ch = bytes[pos];
        if ch == b'\'' || ch == b'"' {
            match memchr::memchr(ch, &bytes[pos + 1..]) {
                None => {
                    return memchr::memchr(b'}', &bytes[pos + 1..]).map(|i| pos + 1 + i + 1);
                }
                Some(i) => {
                    pos = pos + 1 + i + 1;
                    continue;
                }
            }
        }
        if ch == b'}' {
            return Some(pos + 1);
        }
        pos += 1;
    }
    None
}

pub(crate) fn tokenize_param_content(text: &str, start_offset: u32) -> Vec<ParamToken<'_>> {
    use ParamTokenType as T;
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut pos = 0usize;
    let abs = |p: usize| p as u32 + start_offset;

    while pos < bytes.len() {
        let skipped = skip_blanks(bytes, pos);
        if skipped > pos {
            pos = skipped;
            continue;
        }

        if let Some(type_end) = read_type_end(text, pos) {
            tokens.push(ParamToken {
                token_type: T::Type,
                value: &text[pos + 1..type_end - 1],
                start: abs(pos),
                end: abs(type_end),
            });
            pos = type_end;
            continue;
        }

        if bytes[pos] == b'[' {
            if let Some(i) = memchr::memchr(b']', &bytes[pos + 1..]) {
                let end = pos + 1 + i + 1;
                tokens.push(ParamToken {
                    token_type: T::OptionalName,
                    value: &text[pos + 1..end - 1],
                    start: abs(pos),
                    end: abs(end),
                });
                pos = end;
                continue;
            }
        }

        if bytes[pos] == b'-'
            && (pos + 1 >= bytes.len() || bytes[pos + 1] == b' ' || bytes[pos + 1] == b'\t')
        {
            tokens.push(ParamToken {
                token_type: T::Dash,
                value: "-",
                start: abs(pos),
                end: abs(pos + 1),
            });
            pos += 1;
            continue;
        }

        if js::is_word_byte(bytes[pos]) {
            let mut end = pos + 1;
            while end < bytes.len() && (js::is_word_byte(bytes[end]) || bytes[end] == b'-') {
                end += 1;
            }
            tokens.push(ParamToken {
                token_type: T::Word,
                value: &text[pos..end],
                start: abs(pos),
                end: abs(end),
            });
            pos = end;
            continue;
        }

        tokens.push(ParamToken {
            token_type: T::Text,
            value: &text[pos..],
            start: abs(pos),
            end: abs(bytes.len()),
        });
        pos = bytes.len();
    }

    let end = abs(bytes.len());
    tokens.push(ParamToken { token_type: T::EndOfInput, value: "", start: end, end });
    tokens
}
