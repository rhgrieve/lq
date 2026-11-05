//! Coloring for snippets: a small scanner over a whole file, since a line alone doesn't say
//! whether it's inside a tag. Purely cosmetic, so being wrong on odd input is harmless, and
//! it must never fail.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Plain,
    Delimiter,
    Keyword,
    Filter,
    String,
    Number,
    HtmlTag,
    Attribute,
    Comment,
    JsonKey,
}

impl Token {
    pub fn code(self) -> Option<&'static str> {
        match self {
            Token::Plain => None,
            Token::Delimiter => Some("36"),
            Token::Keyword => Some("35"),
            Token::Filter | Token::HtmlTag => Some("34"),
            Token::String => Some("32"),
            Token::Number => Some("33"),
            Token::Attribute | Token::JsonKey => Some("36"),
            Token::Comment => Some("90"),
        }
    }
}

/// Runs of tokens, each starting at a byte.
#[derive(Debug, Default)]
pub struct Highlights(Vec<(u32, Token)>);

impl Highlights {
    pub fn at(&self, byte: usize) -> Token {
        match self.0.partition_point(|(start, _)| *start as usize <= byte) {
            0 => Token::Plain,
            i => self.0[i - 1].1,
        }
    }

    fn mark(&mut self, start: usize, end: usize, token: Token) {
        if end > start {
            self.0.push((start as u32, token));
            self.0.push((end as u32, Token::Plain));
        }
    }
}

pub fn highlight(source: &str, is_json: bool) -> Highlights {
    let mut out = Highlights::default();
    if is_json {
        json(source.as_bytes(), &mut out);
    } else {
        liquid_html(source.as_bytes(), &mut out);
    }
    out
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')
}

fn word_end(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && is_word(s[i]) {
        i += 1;
    }
    i
}

/// The index after the closing quote, or the end of the input.
fn string_end(s: &[u8], i: usize) -> usize {
    let quote = s[i];
    let mut j = i + 1;
    while j < s.len() {
        if s[j] == b'\\' {
            j += 2;
            continue;
        }
        if s[j] == quote {
            return j + 1;
        }
        j += 1;
    }
    s.len()
}

fn find(s: &[u8], from: usize, needle: &[u8]) -> usize {
    s.get(from..).and_then(|rest| rest.windows(needle.len()).position(|w| w == needle)).map_or(s.len(), |p| from + p)
}

fn json(s: &[u8], out: &mut Highlights) {
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'"' => {
                let end = string_end(s, i);
                let mut next = end;
                while next < s.len() && s[next].is_ascii_whitespace() {
                    next += 1;
                }
                let key = s.get(next) == Some(&b':');
                out.mark(i, end, if key { Token::JsonKey } else { Token::String });
                i = end;
            }
            b'/' if s.get(i + 1) == Some(&b'*') => {
                let end = (find(s, i + 2, b"*/") + 2).min(s.len());
                out.mark(i, end, Token::Comment);
                i = end;
            }
            b'/' if s.get(i + 1) == Some(&b'/') => {
                let end = find(s, i, b"\n");
                out.mark(i, end, Token::Comment);
                i = end;
            }
            b'-' | b'0'..=b'9' => {
                let mut end = i + 1;
                while end < s.len() && (s[end].is_ascii_alphanumeric() || matches!(s[end], b'.' | b'+' | b'-')) {
                    end += 1;
                }
                out.mark(i, end, Token::Number);
                i = end;
            }
            b't' | b'f' | b'n' => {
                let end = word_end(s, i);
                if matches!(&s[i..end], b"true" | b"false" | b"null") {
                    out.mark(i, end, Token::Keyword);
                }
                i = end.max(i + 1);
            }
            _ => i += 1,
        }
    }
}

fn liquid_html(s: &[u8], out: &mut Highlights) {
    let mut i = 0;
    let mut in_tag = false;
    while i < s.len() {
        if s[i] == b'{' && matches!(s.get(i + 1), Some(b'{' | b'%')) {
            i = liquid(s, i, out);
            continue;
        }
        if in_tag {
            match s[i] {
                b'>' => {
                    out.mark(i, i + 1, Token::HtmlTag);
                    in_tag = false;
                    i += 1;
                }
                b'/' => {
                    out.mark(i, i + 1, Token::HtmlTag);
                    i += 1;
                }
                b'"' | b'\'' => i = attribute_value(s, i, out),
                b if is_word(b) || b == b':' || b == b'@' => {
                    let mut end = i + 1;
                    while end < s.len() && (is_word(s[end]) || matches!(s[end], b':' | b'.' | b'@')) {
                        end += 1;
                    }
                    out.mark(i, end, Token::Attribute);
                    i = end;
                }
                _ => i += 1,
            }
            continue;
        }
        if s[i] == b'<' {
            if s[i..].starts_with(b"<!--") {
                let end = (find(s, i + 4, b"-->") + 3).min(s.len());
                out.mark(i, end, Token::Comment);
                i = end;
                continue;
            }
            let name = i + 1 + usize::from(s.get(i + 1) == Some(&b'/'));
            if s.get(name).is_some_and(u8::is_ascii_alphabetic) {
                let end = word_end(s, name);
                out.mark(i, end, Token::HtmlTag);
                in_tag = true;
                i = end;
                continue;
            }
        }
        i += 1;
    }
}

/// A quoted attribute value, with any Liquid inside it colored as Liquid.
fn attribute_value(s: &[u8], start: usize, out: &mut Highlights) -> usize {
    let quote = s[start];
    let mut i = start + 1;
    let mut run = start;
    while i < s.len() {
        if s[i] == b'{' && matches!(s.get(i + 1), Some(b'{' | b'%')) {
            out.mark(run, i, Token::String);
            i = liquid(s, i, out);
            run = i;
            continue;
        }
        if s[i] == quote {
            out.mark(run, i + 1, Token::String);
            return i + 1;
        }
        i += 1;
    }
    out.mark(run, s.len(), Token::String);
    s.len()
}

/// `{{ ... }}` or `{% ... %}` from its opening delimiter. Returns the index after it.
fn liquid(s: &[u8], start: usize, out: &mut Highlights) -> usize {
    let is_tag = s[start + 1] == b'%';
    let close: &[u8] = if is_tag { b"%}" } else { b"}}" };
    let mut i = start + 2;
    if s.get(i) == Some(&b'-') {
        i += 1;
    }
    out.mark(start, i, Token::Delimiter);
    while i < s.len() && s[i].is_ascii_whitespace() {
        i += 1;
    }
    if is_tag {
        let end = word_end(s, i);
        out.mark(i, end, if s.get(i) == Some(&b'#') { Token::Comment } else { Token::Keyword });
        if s.get(i) == Some(&b'#') {
            let end = find(s, i, close);
            out.mark(i, end, Token::Comment);
            i = end;
        } else {
            i = end;
        }
    }
    let mut after_pipe = false;
    while i < s.len() {
        if s[i..].starts_with(close) || (s[i] == b'-' && s[i + 1..].starts_with(close)) {
            let end = i + usize::from(s[i] == b'-') + 2;
            out.mark(i, end, Token::Delimiter);
            return end;
        }
        match s[i] {
            b'"' | b'\'' => {
                let end = string_end(s, i).min(s.len());
                out.mark(i, end, Token::String);
                i = end;
            }
            b'|' => {
                after_pipe = true;
                i += 1;
            }
            b'0'..=b'9' => {
                let mut end = i + 1;
                while end < s.len() && (s[end].is_ascii_digit() || s[end] == b'.') {
                    end += 1;
                }
                out.mark(i, end, Token::Number);
                i = end;
            }
            b if is_word(b) => {
                let end = word_end(s, i);
                if after_pipe {
                    out.mark(i, end, Token::Filter);
                    after_pipe = false;
                }
                i = end.max(i + 1);
            }
            _ => i += 1,
        }
    }
    s.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_liquid_in_html() {
        let s = r#"<a href="{{ x | t: 'k' }}">{% if y %}"#;
        let h = highlight(s, false);
        assert_eq!(h.at(0), Token::HtmlTag);
        assert_eq!(h.at(s.find("href").unwrap()), Token::Attribute);
        assert_eq!(h.at(s.find("{{").unwrap()), Token::Delimiter);
        assert_eq!(h.at(s.find("t:").unwrap()), Token::Filter);
        assert_eq!(h.at(s.find("'k'").unwrap()), Token::String);
        assert_eq!(h.at(s.find("if").unwrap()), Token::Keyword);
    }

    #[test]
    fn colors_json() {
        let s = r#"{"a": "b", "c": 1, "d": true}"#;
        let h = highlight(s, true);
        assert_eq!(h.at(1), Token::JsonKey);
        assert_eq!(h.at(6), Token::String);
        assert_eq!(h.at(s.find('1').unwrap()), Token::Number);
        assert_eq!(h.at(s.find("true").unwrap()), Token::Keyword);
    }

    #[test]
    fn survives_truncated_input() {
        for s in ["{{", "{%", "{% if", "<a href=\"", "{{ 'x", "<!--", "{% # x", "{{-", "\"", "<"] {
            highlight(s, false);
            highlight(s, true);
        }
    }
}
