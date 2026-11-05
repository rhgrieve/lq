//! JavaScript string semantics needed for parity: the `/\s/` whitespace set,
//! `.trim()`, `/\w/`, `.slice()` clamping and `.toLowerCase()` comparison.

/// `/\s/` and `String.prototype.trim` share this set. Note it differs from
/// `char::is_whitespace`: U+FEFF is in, U+0085 is out.
#[inline]
pub fn is_ws_char(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\x0B'
            | '\x0C'
            | '\r'
            | ' '
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

#[inline]
pub fn is_ascii_ws(b: u8) -> bool {
    matches!(b, 9..=13 | 32)
}

/// Byte width of the whitespace char starting at byte `pos`, or 0 when the
/// char there is not whitespace. `pos` must be a char boundary or out of range.
#[inline]
pub fn ws_width_at(s: &str, pos: usize) -> usize {
    let bytes = s.as_bytes();
    match bytes.get(pos) {
        None => 0,
        Some(&b) if b < 0x80 => usize::from(is_ascii_ws(b)),
        Some(_) => match s[pos..].chars().next() {
            Some(c) if is_ws_char(c) => c.len_utf8(),
            _ => 0,
        },
    }
}

/// Byte width of the char at `pos`; 0 at or past the end.
#[inline]
pub fn char_width_at(s: &str, pos: usize) -> usize {
    match s.as_bytes().get(pos) {
        None => 0,
        Some(&b) if b < 0x80 => 1,
        Some(&b) if b < 0xE0 => 2,
        Some(&b) if b < 0xF0 => 3,
        Some(_) => 4,
    }
}

#[inline]
pub fn trim_start(s: &str) -> &str {
    let mut pos = 0;
    loop {
        let w = ws_width_at(s, pos);
        if w == 0 {
            return &s[pos..];
        }
        pos += w;
    }
}

#[inline]
pub fn trim_end(s: &str) -> &str {
    match s.as_bytes().last() {
        None => s,
        Some(&b) if b < 0x80 && !is_ascii_ws(b) => s,
        _ => s.trim_end_matches(is_ws_char),
    }
}

#[inline]
pub fn trim(s: &str) -> &str {
    trim_end(trim_start(s))
}

/// `s.search(/\s/)`, as a byte offset.
#[inline]
pub fn find_ws(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < 0x80 {
            if is_ascii_ws(b) {
                return Some(i);
            }
            i += 1;
        } else {
            if ws_width_at(s, i) > 0 {
                return Some(i);
            }
            i += char_width_at(s, i);
        }
    }
    None
}

/// `/\S/.test(s)`.
#[inline]
pub fn has_non_ws(s: &str) -> bool {
    !trim_start(s).is_empty()
}

/// `/^\s+$/.test(s)`.
#[inline]
pub fn is_all_ws_nonempty(s: &str) -> bool {
    !s.is_empty() && !has_non_ws(s)
}

/// `/\w/` without the `u` flag: ASCII only.
#[inline]
pub fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `s.slice(a, b)` for non-negative indexes: clamps, and yields "" when `a >= b`.
/// Never panics.
#[inline]
pub fn slice(s: &str, a: u32, b: u32) -> &str {
    let (a, b) = (a as usize, (b as usize).min(s.len()));
    if a >= b {
        return "";
    }
    s.get(a..b).unwrap_or("")
}

/// `s.toLowerCase() === lower`.
#[inline]
pub fn lower_eq(s: &str, lower: &str) -> bool {
    if s.is_ascii() {
        s.eq_ignore_ascii_case(lower)
    } else {
        s.to_lowercase() == lower
    }
}

/// `list.includes(s.toLowerCase())`.
pub fn lower_in(s: &str, list: &[&str]) -> bool {
    list.iter().any(|item| lower_eq(s, item))
}
