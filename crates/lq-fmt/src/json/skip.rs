//! Prettier's `src/utilities/skip*.js`, on byte offsets. `None` stands for upstream's
//! `false`.

fn skip_chars(text: &str, index: Option<usize>, chars: &[u8], backwards: bool) -> Option<usize> {
    let mut i = index? as isize;
    let bytes = text.as_bytes();
    let len = bytes.len() as isize;
    while i >= 0 && i < len {
        if !chars.contains(&bytes[i as usize]) {
            return Some(i as usize);
        }
        i += if backwards { -1 } else { 1 };
    }
    if i == -1 {
        // `-1` as upstream returns it; callers treat it as "ran off the start".
        return Some(usize::MAX);
    }
    if i == len {
        Some(i as usize)
    } else {
        None
    }
}

pub fn skip_spaces(text: &str, index: Option<usize>, backwards: bool) -> Option<usize> {
    if index == Some(usize::MAX) {
        return index;
    }
    skip_chars(text, index, b" \t", backwards)
}

/// `skipNewline`.
pub fn skip_newline(text: &str, index: Option<usize>, backwards: bool) -> Option<usize> {
    let i = index?;
    if i == usize::MAX {
        return Some(i);
    }
    let bytes = text.as_bytes();
    let at = |n: usize| bytes.get(n).copied();
    if backwards {
        if i >= 1 && at(i - 1) == Some(b'\r') && at(i) == Some(b'\n') {
            return Some(i.wrapping_sub(2));
        }
        if matches!(at(i), Some(b'\n' | b'\r')) {
            return Some(i.wrapping_sub(1));
        }
        if i >= 2 && at(i - 2) == Some(0xE2) && at(i - 1) == Some(0x80) && matches!(at(i), Some(0xA8 | 0xA9)) {
            return Some(i.wrapping_sub(3));
        }
    } else {
        if at(i) == Some(b'\r') && at(i + 1) == Some(b'\n') {
            return Some(i + 2);
        }
        if matches!(at(i), Some(b'\n' | b'\r')) {
            return Some(i + 1);
        }
        if at(i) == Some(0xE2) && at(i + 1) == Some(0x80) && matches!(at(i + 2), Some(0xA8 | 0xA9)) {
            return Some(i + 3);
        }
    }
    Some(i)
}

/// `hasNewline(text, index)`.
pub fn has_newline(text: &str, index: usize) -> bool {
    let n = skip_spaces(text, Some(index), false);
    n != skip_newline(text, n, false)
}

/// `hasNewline(text, index, { backwards: true })`.
pub fn has_newline_backwards(text: &str, index: usize) -> bool {
    let start = index.wrapping_sub(1);
    let n = skip_spaces(text, Some(start), true);
    n != skip_newline(text, n, true)
}

/// `hasNewlineInRange`.
pub fn has_newline_in_range(text: &str, start: usize, end: usize) -> bool {
    text.as_bytes().get(start..end.min(text.len())).is_some_and(|s| s.contains(&b'\n'))
}

/// `skipInlineComment`.
pub fn skip_inline_comment(text: &str, index: Option<usize>) -> Option<usize> {
    let i = index?;
    let bytes = text.as_bytes();
    if bytes.get(i) == Some(&b'/') && bytes.get(i + 1) == Some(&b'*') {
        let mut r = i + 2;
        while r < bytes.len() {
            if bytes[r] == b'*' && bytes.get(r + 1) == Some(&b'/') {
                return Some(r + 2);
            }
            r += 1;
        }
    }
    Some(i)
}

/// `skipTrailingComment`: `//` to the end of the line.
pub fn skip_trailing_comment(text: &str, index: Option<usize>) -> Option<usize> {
    let i = index?;
    let bytes = text.as_bytes();
    if bytes.get(i) == Some(&b'/') && bytes.get(i + 1) == Some(&b'/') {
        let mut r = i;
        while r < bytes.len() && bytes[r] != b'\n' && bytes[r] != b'\r' {
            r += 1;
        }
        return Some(r);
    }
    Some(i)
}

/// `isNextLineEmpty(text, index)`.
pub fn is_next_line_empty(text: &str, start: usize) -> bool {
    let mut previous = None;
    let mut i = Some(start);
    while i != previous {
        previous = i;
        i = skip_chars(text, i, b",; \t", false);
        i = skip_inline_comment(text, i);
        i = skip_spaces(text, i, false);
    }
    i = skip_trailing_comment(text, i);
    i = skip_newline(text, i, false);
    match i {
        Some(n) => has_newline(text, n),
        None => false,
    }
}

/// `isPreviousLineEmpty(text, start)`.
pub fn is_previous_line_empty(text: &str, start: usize) -> bool {
    let mut r = Some(start.wrapping_sub(1));
    r = skip_spaces(text, r, true);
    r = skip_newline(text, r, true);
    r = skip_spaces(text, r, true);
    let n = skip_newline(text, r, true);
    r != n
}
