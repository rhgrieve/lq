//! How `lq check` presents offenses.

pub mod term;
pub mod upstream;

/// Text from a theme can't be allowed to drive the terminal: control characters and
/// bidirectional overrides become U+FFFD. Newlines and tabs become spaces.
pub fn sanitize(text: &str) -> String {
    text.chars().map(|c| if matches!(c, '\n' | '\t') { ' ' } else { clean_char(c) }).collect()
}

pub fn clean_char(c: char) -> char {
    if c.is_control() || matches!(c as u32, 0x202A..=0x202E | 0x2066..=0x2069) {
        '\u{FFFD}'
    } else {
        c
    }
}
