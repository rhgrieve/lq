//! A glob matcher covering what `overrides[].files`, `.editorconfig` sections and CLI
//! patterns use in micromatch and fast-glob: `*`, `**`, `?`, `[...]`, `{a,b}` and a
//! leading `!`. Matching is with `dot: true`.

/// Expands `{a,b}` alternatives, including nested and `{1..3}`-free forms.
fn expand_braces(pattern: &str) -> Vec<String> {
    let bytes = pattern.as_bytes();
    let mut depth = 0;
    let mut open = None;
    let mut commas = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'[' if depth == 0 => {
                if let Some(off) = pattern[i..].find(']') {
                    i += off;
                }
            }
            b'{' => {
                if depth == 0 {
                    open = Some(i);
                }
                depth += 1;
            }
            b',' if depth == 1 => commas.push(i),
            b'}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let start = open.unwrap_or(0);
                    if commas.is_empty() {
                        // `{a}` is literal.
                        open = None;
                        i += 1;
                        continue;
                    }
                    let mut out = Vec::new();
                    let mut from = start + 1;
                    for &c in commas.iter().chain(std::iter::once(&i)) {
                        let candidate =
                            format!("{}{}{}", &pattern[..start], &pattern[from..c], &pattern[i + 1..]);
                        out.extend(expand_braces(&candidate));
                        from = c + 1;
                    }
                    return out;
                }
            }
            _ => {}
        }
        i += 1;
    }
    vec![pattern.to_string()]
}

fn class_matches(class: &[char], c: char, case_insensitive: bool) -> bool {
    let (negated, class) = match class.first() {
        Some('!' | '^') => (true, &class[1..]),
        _ => (false, class),
    };
    let eq = |a: char, b: char| {
        if case_insensitive {
            a.eq_ignore_ascii_case(&b)
        } else {
            a == b
        }
    };
    let mut hit = false;
    let mut i = 0;
    while i < class.len() {
        if i + 2 < class.len() && class[i + 1] == '-' {
            let (lo, hi) = (class[i], class[i + 2]);
            if (lo..=hi).contains(&c)
                || (case_insensitive
                    && ((lo..=hi).contains(&c.to_ascii_lowercase())
                        || (lo..=hi).contains(&c.to_ascii_uppercase())))
            {
                hit = true;
            }
            i += 3;
        } else {
            if eq(class[i], c) {
                hit = true;
            }
            i += 1;
        }
    }
    hit != negated
}

/// Matches one path segment against one pattern segment.
pub(crate) fn segment_matches(p: &[char], s: &[char], ci: bool) -> bool {
    let Some(&first) = p.first() else {
        return s.is_empty();
    };
    match first {
        '*' => {
            let rest = {
                let mut r = p;
                while r.first() == Some(&'*') {
                    r = &r[1..];
                }
                r
            };
            (0..=s.len()).any(|i| segment_matches(rest, &s[i..], ci))
        }
        '?' => !s.is_empty() && segment_matches(&p[1..], &s[1..], ci),
        '[' => {
            if let Some(close) = p.iter().skip(2).position(|c| *c == ']').map(|i| i + 2) {
                !s.is_empty()
                    && class_matches(&p[1..close], s[0], ci)
                    && segment_matches(&p[close + 1..], &s[1..], ci)
            } else {
                !s.is_empty() && s[0] == '[' && segment_matches(&p[1..], &s[1..], ci)
            }
        }
        '\\' if p.len() > 1 => {
            !s.is_empty() && s[0] == p[1] && segment_matches(&p[2..], &s[1..], ci)
        }
        c => {
            !s.is_empty()
                && (c == s[0] || (ci && c.eq_ignore_ascii_case(&s[0])))
                && segment_matches(&p[1..], &s[1..], ci)
        }
    }
}

fn segments_match(p: &[Vec<char>], s: &[Vec<char>], ci: bool) -> bool {
    let Some(first) = p.first() else {
        return s.is_empty();
    };
    if first.len() == 2 && first[0] == '*' && first[1] == '*' {
        return (0..=s.len()).any(|i| segments_match(&p[1..], &s[i..], ci));
    }
    !s.is_empty() && segment_matches(first, &s[0], ci) && segments_match(&p[1..], &s[1..], ci)
}

fn split_segments(s: &str) -> Vec<Vec<char>> {
    s.split('/').map(|seg| seg.chars().collect()).collect()
}

/// A compiled pattern.
#[derive(Clone, Debug)]
pub struct Glob {
    alternatives: Vec<Vec<Vec<char>>>,
    negated: bool,
}

impl Glob {
    pub fn new(pattern: &str) -> Self {
        let (negated, pattern) = match pattern.strip_prefix('!') {
            Some(rest) if !rest.starts_with('(') => (true, rest),
            _ => (false, pattern),
        };
        let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
        Glob {
            alternatives: expand_braces(pattern).iter().map(|a| split_segments(a)).collect(),
            negated,
        }
    }

    fn matches_raw(&self, path: &str) -> bool {
        let segments = split_segments(path);
        self.alternatives.iter().any(|a| segments_match(a, &segments, false))
    }

    /// `micromatch.isMatch(path, pattern, { dot: true })`.
    pub fn is_match(&self, path: &str) -> bool {
        self.matches_raw(path) != self.negated
    }

    /// `micromatch.isMatch(path, pattern, { basename: true, dot: true })`.
    pub fn is_match_basename(&self, path: &str) -> bool {
        let base = path.rsplit('/').next().unwrap_or(path);
        self.matches_raw(base) != self.negated
    }
}

/// `pathMatchesGlobs`. Each `isMatch` call compiles `excludedPatterns` with its own
/// options, so in the slash-free group an exclude is tested against the base name only.
pub fn path_matches_globs(path: &str, patterns: &[String], excluded: &[String]) -> bool {
    let is_match = |want_slash: bool| {
        let test = |g: &Glob| if want_slash { g.is_match(path) } else { g.is_match_basename(path) };
        !excluded.iter().any(|e| test(&Glob::new(e)))
            && patterns
                .iter()
                .filter(|p| p.contains('/') == want_slash)
                .any(|p| test(&Glob::new(p)))
    };
    is_match(false) || is_match(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(Glob::new("*.liquid").is_match_basename("sections/a.liquid"));
        assert!(!Glob::new("*.liquid").is_match("sections/a.liquid"));
        assert!(Glob::new("**/*.liquid").is_match("a.liquid"));
        assert!(Glob::new("**/*.liquid").is_match("a/b/c.liquid"));
        assert!(Glob::new("a/{b,c}/*.js").is_match("a/c/x.js"));
        assert!(Glob::new("*.{css,js}").is_match("x.js"));
        assert!(Glob::new("[ab].js").is_match("a.js"));
        assert!(!Glob::new("[!ab].js").is_match("a.js"));
        assert!(Glob::new("**").is_match(".hidden/x"));
    }

    #[test]
    fn overrides() {
        let p = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(path_matches_globs("snippets/a.liquid", &p(&["*.liquid"]), &[]));
        assert!(!path_matches_globs("snippets/a.css", &p(&["*.liquid"]), &[]));
        assert!(path_matches_globs("snippets/a.liquid", &p(&["snippets/*.liquid"]), &[]));
        assert!(!path_matches_globs("a.liquid", &p(&["*.liquid"]), &p(&["a.*"])));
        // A slash in the exclude never matches a base name.
        assert!(path_matches_globs("sub/a.liquid", &p(&["*.liquid"]), &p(&["sub/*.liquid"])));
    }
}
