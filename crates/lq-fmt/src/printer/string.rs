//! Port of `printer/utils/string.ts`, plus the JavaScript string semantics it relies on.

/// JavaScript `\s` / `String.prototype.trim` whitespace. Differs from Rust's
/// `char::is_whitespace`: includes U+FEFF, excludes U+0085.
#[inline]
pub fn is_js_ws(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0B}'
            | '\u{0C}'
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
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_ws)
}

#[inline]
pub fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_ws)
}

#[inline]
pub fn js_trim_start(s: &str) -> &str {
    s.trim_start_matches(is_js_ws)
}

/// `source.slice(start, end)` with byte offsets, never panicking. Offsets computed from
/// rebuilt marker strings can land mid-character; they round down to a boundary.
pub fn slice(source: &str, start: usize, end: usize) -> &str {
    let mut start = start.min(source.len());
    let mut end = end.min(source.len());
    if start >= end {
        return "";
    }
    while !source.is_char_boundary(start) {
        start -= 1;
    }
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    &source[start..end]
}

/// `isWhitespace(source, loc)` for the character starting at byte `loc`.
#[inline]
pub fn is_whitespace_at(source: &str, loc: usize) -> bool {
    match source.get(loc..).and_then(|s| s.chars().next()) {
        Some(c) => is_js_ws(c),
        None => false,
    }
}

/// `isWhitespace(source, loc - 1)` for the character ending at byte `loc`.
#[inline]
pub fn is_whitespace_before(source: &str, loc: usize) -> bool {
    match source.get(..loc).and_then(|s| s.chars().next_back()) {
        Some(c) => is_js_ws(c),
        None => false,
    }
}

/// `str.replace(/^(?: |\t)*(\r?\n)*|\s*$/g, '').split(/\r?\n/)`.
pub fn body_lines(s: &str) -> Vec<&str> {
    split_lines(strip_body(s))
}

fn strip_body(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut p = 0;
    while p < bytes.len() && (bytes[p] == b' ' || bytes[p] == b'\t') {
        p += 1;
    }
    loop {
        if p < bytes.len() && bytes[p] == b'\n' {
            p += 1;
        } else if p + 1 < bytes.len() && bytes[p] == b'\r' && bytes[p + 1] == b'\n' {
            p += 2;
        } else {
            break;
        }
    }
    // After an empty first match the next search starts one unit in, so the first
    // character cannot be consumed by `\s*$`.
    let floor = if p == 0 {
        s.chars().next().map_or(0, char::len_utf8)
    } else {
        p
    };
    let end = js_trim_end(s).len().max(floor);
    &s[p..end]
}

/// `split(/\r?\n/)`.
pub fn split_lines(s: &str) -> Vec<&str> {
    s.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect()
}

/// `markup.trim().split('\n')`.
pub fn markup_lines(markup: &str) -> Vec<&str> {
    js_trim(markup).split('\n').collect()
}

/// `reindent(lines, skipFirst)`. Results are subslices of the input lines.
pub fn reindent<'a>(lines: &[&'a str], skip_first: bool) -> Vec<&'a str> {
    let mut min_indent = usize::MAX;
    for (i, line) in lines.iter().enumerate() {
        if skip_first && i == 0 {
            continue;
        }
        if js_trim(line).is_empty() {
            continue;
        }
        let n = line.chars().take_while(|c| is_js_ws(*c)).count();
        min_indent = min_indent.min(n);
    }
    if min_indent == usize::MAX {
        return lines.to_vec();
    }
    lines
        .iter()
        .map(|line| {
            // `^\s{n}`: unchanged when the line has fewer leading whitespace characters.
            let mut stripped = *line;
            let mut chars = line.char_indices();
            let mut count = 0;
            while count < min_indent {
                match chars.next() {
                    Some((_, c)) if is_js_ws(c) => count += 1,
                    _ => break,
                }
            }
            if count == min_indent {
                let offset = chars.next().map_or(line.len(), |(i, _)| i);
                stripped = &line[offset..];
            }
            js_trim_end(stripped)
        })
        .collect()
}

/// `hasLineBreakInRange`.
#[inline]
pub fn has_line_break_in_range(source: &str, start: usize, end: usize) -> bool {
    if start >= source.len() {
        return false;
    }
    match memchr::memchr(b'\n', &source.as_bytes()[start..]) {
        Some(i) => start + i < end,
        None => false,
    }
}

/// `hasMoreThanOneNewLineBetweenNodes` on already resolved offsets.
pub fn has_more_than_one_newline_between(source: &str, start: usize, end: usize) -> bool {
    memchr::memchr_iter(b'\n', slice(source, start, end).as_bytes()).count() > 1
}

/// `.length` of a string in UTF-16 units.
pub fn utf16_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.encode_utf16().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_lines_strips_like_the_regex() {
        assert_eq!(body_lines("   foo\nbar  \n"), ["foo", "bar"]);
        assert_eq!(body_lines("  \n\n  foo\n  bar"), ["  foo", "  bar"]);
        assert_eq!(body_lines("\t\n \n"), [""]);
        assert_eq!(body_lines("\u{0C}"), ["\u{0C}"]);
        assert_eq!(body_lines(""), [""]);
    }

    #[test]
    fn reindent_strips_common_indent() {
        assert_eq!(reindent(&["a", "    b", "  c "], true), ["a", "  b", "c"]);
        assert_eq!(reindent(&["  a", ""], false), ["a", ""]);
        assert_eq!(reindent(&["  a  "], true), ["  a  "]);
    }

    #[test]
    fn whitespace_lookups_are_char_aware() {
        assert!(is_whitespace_before("\u{A0}x", 2));
        assert!(!is_whitespace_before("éx", 2));
        assert!(is_whitespace_at("x y", 1));
        assert!(!is_whitespace_before("x", 0));
    }
}
