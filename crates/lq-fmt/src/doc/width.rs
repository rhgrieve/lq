//! Port of Prettier's `getStringWidth`.

use super::width_data::{EMOJI_REGEX, FULLWIDTH_RANGES, NARROW_EMOJI_REGEX, WIDE_RANGES};
use regex::{Regex, RegexBuilder};
use std::sync::OnceLock;

fn compile(pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .size_limit(1 << 28)
        .dfa_size_limit(1 << 24)
        .build()
        .expect("generated emoji pattern compiles")
}

fn emoji_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| compile(EMOJI_REGEX))
}

fn narrow_emoji_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| compile(NARROW_EMOJI_REGEX))
}

fn is_in_range(ranges: &[u32], code_point: u32) -> bool {
    let mut low = 0isize;
    let mut high = (ranges.len() / 2) as isize - 1;
    while low <= high {
        let mid = (low + high) / 2;
        let i = (mid * 2) as usize;
        if code_point < ranges[i] {
            high = mid - 1;
        } else if code_point > ranges[i + 1] {
            low = mid + 1;
        } else {
            return true;
        }
    }
    false
}

#[inline]
fn is_full_width(cp: u32) -> bool {
    (12288..=65510).contains(&cp) && is_in_range(FULLWIDTH_RANGES, cp)
}

#[inline]
fn is_wide(cp: u32) -> bool {
    // Upstream's fast path: the range holding the common CJK block.
    if (12880..=42124).contains(&cp) {
        return true;
    }
    (4352..=262141).contains(&cp) && is_in_range(WIDE_RANGES, cp)
}

#[inline]
fn char_width(cp: u32) -> usize {
    if cp <= 31 || (127..=159).contains(&cp) {
        return 0;
    }
    if (768..=879).contains(&cp) || (65024..=65039).contains(&cp) {
        return 0;
    }
    if is_full_width(cp) || is_wide(cp) {
        2
    } else {
        1
    }
}

/// Whether the emoji regex could match: every alternative starts with one of these or
/// needs U+20E3. Skipping it keeps accented text and punctuation cheap.
#[inline]
fn may_contain_emoji(c: char) -> bool {
    let cp = c as u32;
    cp == 0xA9 || cp == 0xAE || cp >= 0x203C
}

/// `getStringWidth`.
pub fn get_string_width(text: &str) -> usize {
    if text.bytes().all(|b| (0x20..=0x7F).contains(&b)) {
        return text.len();
    }
    if !text.chars().any(may_contain_emoji) {
        return text.chars().map(|c| char_width(c as u32)).sum();
    }
    // Map to the surrogate-preserving form the generated patterns expect.
    let mut mapped = String::with_capacity(text.len());
    for c in text.chars() {
        if (c as u32) < 0x10000 {
            mapped.push(c);
        } else {
            let mut buf = [0u16; 2];
            for u in c.encode_utf16(&mut buf) {
                mapped.push(char::from_u32(0xE0000 + (*u as u32 - 0xD800)).unwrap());
            }
        }
    }
    let mut width = 0;
    let mut last = 0;
    for m in emoji_regex().find_iter(&mapped) {
        width += segment_width(&mapped[last..m.start()]);
        width += if narrow_emoji_regex().is_match(m.as_str()) { 1 } else { 2 };
        last = m.end();
    }
    width + segment_width(&mapped[last..])
}

/// Sums the widths of a mapped segment, recombining surrogate pairs.
fn segment_width(segment: &str) -> usize {
    let mut width = 0;
    let mut high: Option<u32> = None;
    for c in segment.chars() {
        let cp = c as u32;
        if (0xE0000..=0xE07FF).contains(&cp) {
            let unit = cp - 0xE0000 + 0xD800;
            match high.take() {
                Some(h) => {
                    width += char_width(0x10000 + ((h - 0xD800) << 10) + (unit - 0xDC00));
                }
                None => high = Some(unit),
            }
        } else {
            width += char_width(cp);
        }
    }
    width
}

#[cfg(test)]
mod tests {
    use super::get_string_width as w;

    #[test]
    fn widths() {
        assert_eq!(w("abc"), 3);
        assert_eq!(w("é"), 1);
        assert_eq!(w("e\u{301}"), 1);
        assert_eq!(w("日本語"), 6);
        assert_eq!(w("\t"), 0);
        assert_eq!(w("👍"), 2);
        assert_eq!(w("©"), 1);
        assert_eq!(w("1\u{FE0F}\u{20E3}"), 2);
        assert_eq!(w("🇨🇦"), 2);
        assert_eq!(w("𝒳"), 1);
    }
}
