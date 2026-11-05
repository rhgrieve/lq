//! Port of `vscode-uri` (POSIX behaviour) plus theme-check's `path.ts`.

use std::borrow::Cow;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uri {
    pub scheme: String,
    pub authority: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

/// `[UriError]`s thrown by `URI.parse`/`with`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UriError(pub &'static str);

fn valid_scheme(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && (b[0].is_ascii_alphanumeric() || b[0] == b'_')
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'+' | b'.' | b'-'))
}

fn reference_resolution(scheme: &str, path: String) -> String {
    match scheme {
        "https" | "http" | "file" => {
            if path.is_empty() {
                "/".into()
            } else if !path.starts_with('/') {
                format!("/{path}")
            } else {
                path
            }
        }
        _ => path,
    }
}

impl Uri {
    /// The `new Uri(...)` constructor: scheme fix, validation, reference resolution.
    pub fn new(
        scheme: &str,
        authority: String,
        path: String,
        query: String,
        fragment: String,
    ) -> Result<Uri, UriError> {
        let scheme = if scheme.is_empty() { "file" } else { scheme };
        let path = reference_resolution(scheme, path);
        if !valid_scheme(scheme) {
            return Err(UriError("[UriError]: Scheme contains illegal characters."));
        }
        if !path.is_empty() {
            if !authority.is_empty() {
                if !path.starts_with('/') {
                    return Err(UriError("[UriError]: If a URI contains an authority component, then the path component must either be empty or begin with a slash (\"/\") character"));
                }
            } else if path.starts_with("//") {
                return Err(UriError("[UriError]: If a URI does not contain an authority component, then the path cannot begin with two slash characters (\"//\")"));
            }
        }
        Ok(Uri { scheme: scheme.to_string(), authority, path, query, fragment })
    }

    /// `URI.parse(value)`.
    pub fn parse(value: &str) -> Result<Uri, UriError> {
        // /^(([^:/?#]+?):)?(\/\/([^/?#]*))?([^?#]*)(\?([^#]*))?(#(.*))?/ (`.` stops
        // at line terminators; the regex is unanchored at its end).
        let (mut rest, scheme) = match value.find([':', '/', '?', '#']) {
            Some(i) if i > 0 && value.as_bytes()[i] == b':' => (&value[i + 1..], &value[..i]),
            _ => (value, ""),
        };
        let mut authority = "";
        if let Some(r) = rest.strip_prefix("//") {
            let end = r.find(['/', '?', '#']).unwrap_or(r.len());
            authority = &r[..end];
            rest = &r[end..];
        }
        let end = rest.find(['?', '#']).unwrap_or(rest.len());
        let path = &rest[..end];
        rest = &rest[end..];
        let mut query = "";
        if let Some(r) = rest.strip_prefix('?') {
            let end = r.find('#').unwrap_or(r.len());
            query = &r[..end];
            rest = &r[end..];
        }
        let mut fragment = "";
        if let Some(r) = rest.strip_prefix('#') {
            let end = r.find(['\n', '\r', '\u{2028}', '\u{2029}']).unwrap_or(r.len());
            fragment = &r[..end];
        }
        Uri::new(
            scheme,
            percent_decode(authority).into_owned(),
            percent_decode(path).into_owned(),
            percent_decode(query).into_owned(),
            percent_decode(fragment).into_owned(),
        )
    }

    /// `URI.file(path)`.
    pub fn file(path: &str) -> Uri {
        let (authority, path) = if let Some(rest) = path.strip_prefix("//") {
            match rest.find('/') {
                None => (rest.to_string(), "/".to_string()),
                Some(i) => (rest[..i].to_string(), rest[i..].to_string()),
            }
        } else {
            (String::new(), path.to_string())
        };
        let path = reference_resolution("file", path);
        Uri { scheme: "file".into(), authority, path, query: String::new(), fragment: String::new() }
    }

    /// `uri.toString(skipEncoding)`.
    pub fn to_string_with(&self, skip_encoding: bool) -> String {
        let enc = |s: &str, is_path: bool, is_authority: bool| -> String {
            if skip_encoding {
                encode_minimal(s).into_owned()
            } else {
                encode_fast(s, is_path, is_authority)
            }
        };
        let mut res = String::with_capacity(self.path.len() + 8);
        res.push_str(&self.scheme);
        res.push(':');
        if !self.authority.is_empty() || self.scheme == "file" {
            res.push_str("//");
        }
        if !self.authority.is_empty() {
            let mut authority: &str = &self.authority;
            if let Some(idx) = authority.find('@') {
                let userinfo = &authority[..idx];
                authority = &authority[idx + 1..];
                match userinfo.rfind(':') {
                    None => res.push_str(&enc(userinfo, false, false)),
                    Some(i) => {
                        res.push_str(&enc(&userinfo[..i], false, false));
                        res.push(':');
                        res.push_str(&enc(&userinfo[i + 1..], false, true));
                    }
                }
                res.push('@');
            }
            let authority = authority.to_lowercase();
            match authority.rfind(':') {
                None => res.push_str(&enc(&authority, false, true)),
                Some(i) => {
                    res.push_str(&enc(&authority[..i], false, true));
                    res.push_str(&authority[i..]);
                }
            }
        }
        if !self.path.is_empty() {
            let b = self.path.as_bytes();
            let lowered;
            let path: &str = if b.len() >= 3 && b[0] == b'/' && b[2] == b':' && b[1].is_ascii_uppercase() {
                lowered = format!("/{}:{}", (b[1] + 32) as char, &self.path[3..]);
                &lowered
            } else if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_uppercase() {
                lowered = format!("{}:{}", (b[0] + 32) as char, &self.path[2..]);
                &lowered
            } else {
                &self.path
            };
            res.push_str(&enc(path, true, false));
        }
        if !self.query.is_empty() {
            res.push('?');
            res.push_str(&enc(&self.query, false, false));
        }
        if !self.fragment.is_empty() {
            res.push('#');
            if skip_encoding {
                res.push_str(&self.fragment);
            } else {
                res.push_str(&encode_fast(&self.fragment, false, false));
            }
        }
        res
    }

    /// `uri.fsPath` on POSIX.
    pub fn fs_path(&self) -> String {
        let b = self.path.as_bytes();
        if !self.authority.is_empty() && self.path.len() > 1 && self.scheme == "file" {
            format!("//{}{}", self.authority, self.path)
        } else if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
            format!("{}{}", (b[1] as char).to_ascii_lowercase(), &self.path[2..])
        } else {
            self.path.clone()
        }
    }

    fn with_path(&self, path: String) -> Result<Uri, UriError> {
        Uri::new(&self.scheme, self.authority.clone(), path, self.query.clone(), self.fragment.clone())
    }
}

fn percent_decode(s: &str) -> Cow<'_, str> {
    if !s.contains('%') {
        return Cow::Borrowed(s);
    }
    // /(%[0-9A-Za-z][0-9A-Za-z])+/g, each run decoded gracefully.
    let b = s.as_bytes();
    let is_token = |i: usize| {
        i + 2 < b.len()
            && b[i] == b'%'
            && b[i + 1].is_ascii_alphanumeric()
            && b[i + 2].is_ascii_alphanumeric()
    };
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut start = 0;
    while i < b.len() {
        if is_token(i) {
            out.push_str(&s[start..i]);
            let run_start = i;
            while is_token(i) {
                i += 3;
            }
            decode_graceful(&s[run_start..i], &mut out);
            start = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&s[start..]);
    Cow::Owned(out)
}

/// `decodeURIComponentGraceful`: on failure, keep the first `%XX` and retry the rest.
fn decode_graceful(mut s: &str, out: &mut String) {
    loop {
        match decode_uri_component(s) {
            Some(d) => {
                out.push_str(&d);
                return;
            }
            None => {
                if s.len() > 3 {
                    out.push_str(&s[..3]);
                    s = &s[3..];
                } else {
                    out.push_str(s);
                    return;
                }
            }
        }
    }
}

/// `decodeURIComponent`; `None` when it would throw.
pub fn decode_uri_component(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut bytes = Vec::with_capacity(b.len());
    let mut i = 0;
    let hex = |c: u8| (c as char).to_digit(16);
    while i < b.len() {
        if b[i] == b'%' {
            let hi = hex(*b.get(i + 1)?)?;
            let lo = hex(*b.get(i + 2)?)?;
            bytes.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            bytes.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(bytes).ok()
}

/// `encodeURIComponentMinimal`: only `#` and `?`.
fn encode_minimal(path: &str) -> Cow<'_, str> {
    if !path.contains(['#', '?']) {
        return Cow::Borrowed(path);
    }
    Cow::Owned(path.replace('#', "%23").replace('?', "%3F"))
}

/// `encodeURIComponentFast`.
fn encode_fast(s: &str, is_path: bool, is_authority: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let keep = c.is_ascii_alphanumeric()
            || matches!(c, '-' | '.' | '_' | '~')
            || (is_path && c == '/')
            || (is_authority && matches!(c, '[' | ']' | ':'));
        if keep {
            out.push(c);
            continue;
        }
        let mut buf = [0u8; 4];
        match encode_table(c) {
            Some(t) => out.push_str(t),
            None => {
                for byte in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    out
}

fn encode_table(c: char) -> Option<&'static str> {
    Some(match c {
        ':' => "%3A",
        '/' => "%2F",
        '?' => "%3F",
        '#' => "%23",
        '[' => "%5B",
        ']' => "%5D",
        '@' => "%40",
        '!' => "%21",
        '$' => "%24",
        '&' => "%26",
        '\'' => "%27",
        '(' => "%28",
        ')' => "%29",
        '*' => "%2A",
        '+' => "%2B",
        ',' => "%2C",
        ';' => "%3B",
        '=' => "%3D",
        ' ' => "%20",
        _ => return None,
    })
}

/// `posix.normalize` over the already joined string.
pub fn posix_normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let absolute = path.starts_with('/');
    let trailing = path.ends_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let mut out = parts.join("/");
    if out.is_empty() && !absolute {
        out.push('.');
    }
    if !out.is_empty() && trailing {
        out.push('/');
    }
    if absolute {
        out.insert(0, '/');
    }
    out
}

/// `posix.join(...parts)`.
pub fn posix_join<S: AsRef<str>>(parts: &[S]) -> String {
    let mut joined = String::new();
    for p in parts {
        let p = p.as_ref();
        if p.is_empty() {
            continue;
        }
        if !joined.is_empty() {
            joined.push('/');
        }
        joined.push_str(p);
    }
    if joined.is_empty() {
        return ".".into();
    }
    posix_normalize(&joined)
}

/// `posix.dirname(path)`.
pub fn posix_dirname(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let b = path.as_bytes();
    let has_root = b[0] == b'/';
    let mut end = None;
    let mut matched_slash = true;
    for i in (1..b.len()).rev() {
        if b[i] == b'/' {
            if !matched_slash {
                end = Some(i);
                break;
            }
        } else {
            matched_slash = false;
        }
    }
    match end {
        None => (if has_root { "/" } else { "." }).into(),
        Some(1) if has_root => "//".into(),
        Some(e) => path[..e].to_string(),
    }
}

/// `posix.basename(path)`.
pub fn posix_basename(path: &str) -> &str {
    let t = path.trim_end_matches('/');
    match t.rfind('/') {
        Some(i) => &t[i + 1..],
        None => t,
    }
}

/// `posix.resolve(...)` for an absolute first segment (no cwd involved).
fn posix_resolve(base: &str, rel: &str) -> String {
    let joined =
        if rel.starts_with('/') { rel.to_string() } else { format!("{base}/{rel}") };
    let n = posix_normalize(&joined);
    if n.len() > 1 && n.ends_with('/') {
        n[..n.len() - 1].to_string()
    } else {
        n
    }
}

// ---- `path.ts` -----------------------------------------------------------

/// `path.normalize(uri)`. An unparseable URI (which throws upstream) is returned as is.
pub fn normalize(uri: &str) -> String {
    match Uri::parse(uri) {
        Ok(u) => normalize_uri(&u),
        Err(_) => uri.to_string(),
    }
}

/// `path.normalize(uri: URI)`.
pub fn normalize_uri(uri: &Uri) -> String {
    let s = uri.to_string_with(true);
    if s.contains('\\') {
        s.replace('\\', "/")
    } else {
        s
    }
}

/// `path.relative(uri, rootUri)`.
pub fn relative(uri: &str, root_uri: &str) -> String {
    let normalized = normalize(uri);
    // `.replace(rootUri, '')`: first occurrence only.
    let replaced: Cow<'_, str> = match normalized.find(root_uri) {
        Some(i) => {
            let mut s = String::with_capacity(normalized.len());
            s.push_str(&normalized[..i]);
            s.push_str(&normalized[i + root_uri.len()..]);
            Cow::Owned(s)
        }
        None => Cow::Borrowed(&normalized),
    };
    let replaced = replaced.replace("\\\\", "/");
    replaced.trim_start_matches('/').to_string()
}

/// `path.join(rootUri, ...paths)`.
pub fn join<S: AsRef<str>>(root_uri: &str, paths: &[S]) -> String {
    let Ok(uri) = Uri::parse(root_uri) else { return root_uri.to_string() };
    let mut parts: Vec<&str> = Vec::with_capacity(paths.len() + 1);
    parts.push(&uri.path);
    parts.extend(paths.iter().map(AsRef::as_ref));
    match uri.with_path(posix_join(&parts)) {
        Ok(u) => normalize_uri(&u),
        Err(_) => root_uri.to_string(),
    }
}

/// `path.resolve(uri, path)` (`Utils.resolvePath`).
pub fn resolve(uri: &str, path: &str) -> String {
    let Ok(u) = Uri::parse(uri) else { return uri.to_string() };
    let mut base = u.path.clone();
    let mut slash_added = false;
    if !base.starts_with('/') {
        base.insert(0, '/');
        slash_added = true;
    }
    let mut resolved = posix_resolve(&base, path);
    if slash_added && resolved.starts_with('/') && u.authority.is_empty() {
        resolved.remove(0);
    }
    match u.with_path(resolved) {
        Ok(r) => normalize_uri(&r),
        Err(_) => uri.to_string(),
    }
}

/// `path.dirname(uri)`.
pub fn dirname(uri: &str) -> String {
    let Ok(u) = Uri::parse(uri) else { return uri.to_string() };
    if u.path.is_empty() || u.path == "/" {
        return normalize_uri(&u);
    }
    let mut p = posix_dirname(&u.path);
    if p == "." {
        p.clear();
    }
    match u.with_path(p) {
        Ok(r) => normalize_uri(&r),
        Err(_) => uri.to_string(),
    }
}

/// `path.basename(uri, ext)`. `ext` must be a literal suffix, as all callers' are.
pub fn basename(uri: &str, ext: Option<&str>) -> String {
    let Ok(u) = Uri::parse(uri) else { return String::new() };
    let base = posix_basename(&u.path);
    match ext {
        Some(e) if !e.is_empty() => base.strip_suffix(e).unwrap_or(base).to_string(),
        _ => base.to_string(),
    }
}

/// `path.fsPath(uri)`.
pub fn fs_path(uri: &str) -> String {
    match Uri::parse(uri) {
        Ok(u) => u.fs_path(),
        Err(_) => uri.to_string(),
    }
}

/// `path.normalize(URI.file(absolutePath))`.
pub fn file_uri(absolute_path: &str) -> String {
    normalize_uri(&Uri::file(absolute_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_root() {
        assert_eq!(normalize("file:/"), "file:///");
        assert_eq!(normalize("file:///a/b c.liquid"), "file:///a/b c.liquid");
        assert_eq!(normalize("file:///a/b%20c.liquid"), "file:///a/b c.liquid");
        assert_eq!(normalize("file:///a/b%23c"), "file:///a/b%23c");
    }

    #[test]
    fn path_helpers() {
        assert_eq!(join("file:///", &["snippets", "a.liquid"]), "file:///snippets/a.liquid");
        assert_eq!(join("file:///root", &["a/../b"]), "file:///root/b");
        assert_eq!(relative("file:///snippets/a.liquid", "file:///"), "snippets/a.liquid");
        assert_eq!(relative("file:///r/snippets/a.liquid", "file:///r"), "snippets/a.liquid");
        assert_eq!(dirname("file:///snippets/a.liquid"), "file:///snippets");
        assert_eq!(dirname("file:///a.liquid"), "file:///");
        assert_eq!(dirname("file:///"), "file:///");
        assert_eq!(basename("file:///snippets/a.liquid", Some(".liquid")), "a");
        assert_eq!(resolve("file:///a/b", "./dist"), "file:///a/b/dist");
        assert_eq!(resolve("file:///a/b", "../c"), "file:///a/c");
        assert_eq!(fs_path("file:///a/b%20c"), "/a/b c");
        assert_eq!(file_uri("/a/b c#d"), "file:///a/b c%23d");
    }

    #[test]
    fn full_encoding() {
        let u = Uri::parse("file:///a b/c?d=e#f").unwrap();
        assert_eq!(u.to_string_with(false), "file:///a%20b/c?d%3De#f");
        assert_eq!(Uri::parse("HTTP://x").unwrap().scheme, "HTTP");
    }

    #[test]
    fn errors() {
        assert!(Uri::parse("1 2:foo").is_err());
        assert!(Uri::parse("foo").is_ok());
        assert_eq!(Uri::parse("foo").unwrap().to_string_with(true), "file:///foo");
    }
}
