//! Byte offset to UTF-16 / line-column conversion. Everything inside the parser
//! is in UTF-8 bytes; this is the only place UTF-16 appears.

/// A resolved location: `line` is 1-indexed, `character` is 0-indexed and in
/// UTF-16 code units from the start of the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineCharacter {
    pub line: u32,
    pub character: u32,
}

pub struct SourceMap<'s> {
    source: &'s str,
    line_starts: Vec<u32>,
    /// Empty for ASCII sources. Otherwise one entry per non-ASCII char: (byte
    /// offset of the char, offset just past it, bytes minus UTF-16 units so far).
    deltas: Vec<(u32, u32, u32)>,
    utf16_len: u32,
}

impl<'s> SourceMap<'s> {
    pub fn new(source: &'s str) -> Self {
        let mut line_starts = vec![0u32];
        for i in memchr::memchr_iter(b'\n', source.as_bytes()) {
            line_starts.push(i as u32 + 1);
        }
        let mut deltas = Vec::new();
        let mut utf16_len = source.len() as u32;
        if !source.is_ascii() {
            let mut delta = 0u32;
            for (i, c) in source.char_indices() {
                let w = c.len_utf8();
                if w > 1 {
                    delta += (w - c.len_utf16()) as u32;
                    deltas.push((i as u32, (i + w) as u32, delta));
                }
            }
            utf16_len -= delta;
        }
        SourceMap { source, line_starts, deltas, utf16_len }
    }

    pub fn source(&self) -> &'s str {
        self.source
    }

    pub fn is_ascii(&self) -> bool {
        self.deltas.is_empty()
    }

    /// `source.length` in JS.
    pub fn utf16_len(&self) -> u32 {
        self.utf16_len
    }

    /// The UTF-16 index of a byte offset. An offset inside a multi-byte char
    /// maps to that char's start.
    #[inline]
    pub fn utf16_index(&self, byte: u32) -> u32 {
        if self.deltas.is_empty() {
            return byte;
        }
        let i = self.deltas.partition_point(|&(_, after, _)| after <= byte);
        let byte = match self.deltas.get(i) {
            Some(&(start, _, _)) if start < byte => start,
            _ => byte,
        };
        if i == 0 {
            byte
        } else {
            byte - self.deltas[i - 1].2
        }
    }

    /// 1-indexed line and 0-indexed UTF-16 character of a byte offset. Lines are
    /// split on `\n` only.
    pub fn line_character(&self, byte: u32) -> LineCharacter {
        let line = self.line_starts.partition_point(|&s| s <= byte) - 1;
        let start = self.line_starts[line];
        LineCharacter {
            line: line as u32 + 1,
            character: self.utf16_index(byte) - self.utf16_index(start),
        }
    }

    /// `lineColumn(source).fromIndex(i)` from the `line-column` package, taking
    /// a UTF-16 index: `(line, col)`, both 1-origin, or `None` when out of range.
    pub(crate) fn line_col_of_utf16(&self, index: u32) -> Option<(u32, u32)> {
        if index >= self.utf16_len {
            return None;
        }
        let line = self.line_starts.partition_point(|&s| self.utf16_index(s) <= index) - 1;
        Some((line as u32 + 1, index - self.utf16_index(self.line_starts[line]) + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_identity() {
        let map = SourceMap::new("ab\ncd");
        assert!(map.is_ascii());
        assert_eq!(map.utf16_index(4), 4);
        assert_eq!(map.utf16_len(), 5);
        assert_eq!(map.line_character(0), LineCharacter { line: 1, character: 0 });
        assert_eq!(map.line_character(2), LineCharacter { line: 1, character: 2 });
        assert_eq!(map.line_character(3), LineCharacter { line: 2, character: 0 });
        assert_eq!(map.line_character(5), LineCharacter { line: 2, character: 2 });
    }

    #[test]
    fn multibyte_counts_utf16_units() {
        // e-acute: 2 bytes, 1 unit. Em dash: 3 bytes, 1 unit. Emoji: 4 bytes, 2 units.
        let source = "\u{E9}\u{2014}\u{1F600}x";
        let map = SourceMap::new(source);
        assert!(!map.is_ascii());
        assert_eq!(map.utf16_len(), source.encode_utf16().count() as u32);
        assert_eq!(map.utf16_index(0), 0);
        assert_eq!(map.utf16_index(2), 1);
        assert_eq!(map.utf16_index(5), 2);
        assert_eq!(map.utf16_index(9), 4);
        assert_eq!(map.utf16_index(10), 5);
        // Inside a char maps to its start.
        assert_eq!(map.utf16_index(1), 0);
        assert_eq!(map.utf16_index(7), 2);
    }

    #[test]
    fn matches_a_naive_count_at_every_boundary() {
        let source = "a\u{E9}\n\u{1F600}\u{1F600}\r\n\u{2028}b\n\n\u{3000}";
        let map = SourceMap::new(source);
        for (i, _) in source.char_indices().chain([(source.len(), ' ')]) {
            let prefix = &source[..i];
            assert_eq!(map.utf16_index(i as u32), prefix.encode_utf16().count() as u32);
            let line_start = prefix.rfind('\n').map_or(0, |n| n + 1);
            assert_eq!(
                map.line_character(i as u32),
                LineCharacter {
                    line: prefix.matches('\n').count() as u32 + 1,
                    character: prefix[line_start..].encode_utf16().count() as u32,
                }
            );
        }
    }

    #[test]
    fn only_newline_splits_lines() {
        let map = SourceMap::new("a\rb\u{2028}c\nd");
        assert_eq!(map.line_character(4).line, 1);
        assert_eq!(map.line_character(8).line, 2);
    }

    #[test]
    fn line_column_semantics() {
        let map = SourceMap::new("ab\n\u{1F600}c");
        assert_eq!(map.line_col_of_utf16(0), Some((1, 1)));
        assert_eq!(map.line_col_of_utf16(2), Some((1, 3)));
        assert_eq!(map.line_col_of_utf16(3), Some((2, 1)));
        assert_eq!(map.line_col_of_utf16(5), Some((2, 3)));
        assert_eq!(map.line_col_of_utf16(6), None);
        assert_eq!(SourceMap::new("").line_col_of_utf16(0), None);
    }
}
