//! `utils/glob.js` with `{ extended: true, globstar: true }`.

use crate::regex::JsRegex;

#[derive(Debug)]
pub(crate) struct Glob {
    regex: JsRegex,
    /// `**/dir/name` where `name` has at most one `*`: every document checks every
    /// pattern, so these skip the backtracking engine.
    simple: Option<Simple>,
}

#[derive(Debug)]
struct Simple {
    /// `dir/` or empty.
    directory: String,
    prefix: String,
    /// The part after the `*`, if there is one.
    suffix: Option<String>,
}

impl Simple {
    fn parse(glob: &str) -> Option<Simple> {
        let rest = glob.strip_prefix("**/")?;
        let plain = |s: &str| {
            s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        };
        let (directory, name) = match rest.rfind('/') {
            Some(i) => (&rest[..=i], &rest[i + 1..]),
            None => ("", rest),
        };
        if !directory
            .strip_suffix('/')
            .unwrap_or("")
            .bytes()
            .all(|b| b != b'/')
        {
            return None;
        }
        if !plain(directory.strip_suffix('/').unwrap_or("")) {
            return None;
        }
        let (prefix, suffix) = match name.split_once('*') {
            Some((p, s)) => (p, Some(s)),
            None => (name, None),
        };
        if !plain(prefix) || !suffix.is_none_or(plain) {
            return None;
        }
        Some(Simple {
            directory: directory.to_string(),
            prefix: prefix.to_string(),
            suffix: suffix.map(str::to_string),
        })
    }

    /// The regex is `^((?:[^/]*(?:/|$))*)dir/name$`: the group matches any prefix that
    /// is empty or ends in `/`, or the whole text.
    fn is_match(&self, file_name: &str) -> bool {
        let start = file_name.rfind('/').map_or(0, |i| i + 1);
        let (head, name) = file_name.split_at(start);
        let name_matches = match &self.suffix {
            None => name == self.prefix,
            Some(suffix) => {
                name.len() >= self.prefix.len() + suffix.len()
                    && name.starts_with(&self.prefix)
                    && name.ends_with(suffix.as_str())
            }
        };
        if !name_matches {
            return false;
        }
        match head.strip_suffix(self.directory.as_str()) {
            Some(before) => before.is_empty() || before.ends_with('/'),
            None => false,
        }
    }
}

/// `createRegex(glob, opts)`'s regex source.
fn create_regex_source(glob: &str) -> String {
    let chars: Vec<char> = glob.chars().collect();
    let mut re = String::with_capacity(glob.len() * 2);
    let mut in_group = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '/' | '$' | '^' | '+' | '.' | '(' | ')' | '=' | '!' | '|' => {
                re.push('\\');
                re.push(c);
            }
            '?' => re.push('.'),
            '[' | ']' => re.push(c),
            '{' => {
                in_group = true;
                re.push('(');
            }
            '}' => {
                in_group = false;
                re.push(')');
            }
            ',' => {
                if in_group {
                    re.push('|');
                } else {
                    re.push_str("\\,");
                }
            }
            '*' => {
                let prev = i.checked_sub(1).map(|p| chars[p]);
                let mut stars = 1;
                while chars.get(i + 1) == Some(&'*') {
                    stars += 1;
                    i += 1;
                }
                let next = chars.get(i + 1).copied();
                let globstar = stars > 1
                    && matches!(prev, Some('/' | '{' | ',') | None)
                    && matches!(next, Some('/' | ',' | '}') | None);
                if globstar {
                    if next == Some('/') {
                        i += 1;
                    } else if prev == Some('/') && re.ends_with("\\/") {
                        re.truncate(re.len() - 2);
                    }
                    re.push_str("((?:[^/]*(?:/|$))*)");
                } else {
                    re.push_str("([^/]*)");
                }
            }
            _ => re.push(c),
        }
        i += 1;
    }
    format!("^{re}$")
}

impl Glob {
    /// `Err` where `new RegExp` throws.
    pub(crate) fn new(glob: &str) -> Result<Glob, ()> {
        let regex = JsRegex::new(&create_regex_source(glob), "").map_err(|_| ())?;
        Ok(Glob {
            regex,
            simple: Simple::parse(glob),
        })
    }

    pub(crate) fn is_match(&self, file_name: &str) -> bool {
        match &self.simple {
            Some(simple) => simple.is_match(file_name),
            None => self.regex.is_match(file_name),
        }
    }
}
