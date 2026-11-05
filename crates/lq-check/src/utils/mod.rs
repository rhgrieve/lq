//! `utils/*`.

pub mod block;
pub mod file;
pub mod styles;
pub mod traversal;

/// `s.length`: UTF-16 units. For parity in messages and thresholds.
pub fn js_len(s: &str) -> usize {
    lq_json::js::utf16_len(s)
}

/// `getOffset(source, line, column)` with 1-based `line` and `column` in UTF-16 units:
/// the byte offset, or `None` for `-1`. `lineColumn(source).toIndex`.
pub fn offset_of_line_column(source: &str, line: u32, column: u32) -> Option<u32> {
    if line == 0 || column == 0 {
        return None;
    }
    let mut start = 0usize;
    let mut lines = source.split('\n');
    let mut text = lines.next()?;
    for _ in 1..line {
        start += text.len() + 1;
        text = lines.next()?;
    }
    let mut units = 1;
    for (i, c) in text.char_indices() {
        if units >= column {
            return (units == column).then_some((start + i) as u32);
        }
        units += c.len_utf16() as u32;
    }
    None
}
