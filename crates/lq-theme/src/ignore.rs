//! A port of theme-check-common's `isIgnored`.

use crate::config::Config;
use crate::minimatch::{split_path, Minimatch};

/// `isIgnored(uri, config, checkDef)`, where `check_code` is `checkDef.meta.code`.
/// `uri` may be any string: the file walk passes absolute paths, not URIs, so a
/// `/rooted` pattern (rewritten to a `file://` pattern) cannot match there.
pub fn is_ignored(uri: &str, config: &Config, check_code: Option<&str>) -> bool {
    let check = check_code.and_then(|c| config.check_ignore_matchers(c));
    let global = config.global_ignore_matchers();
    if check.is_none_or(<[Minimatch]>::is_empty) && global.is_empty() {
        return false;
    }
    let parts = split_path(uri);
    let hit = |m: &Minimatch| m.matches_parts(&parts);
    check.is_some_and(|c| c.iter().any(hit)) || global.iter().any(hit)
}

/// The rewriting upstream applies to every ignore pattern.
pub(crate) fn rewrite_pattern(pattern: &str, root_uri: &str) -> String {
    let mut p = match pattern.strip_prefix('/') {
        // "absolute patterns" are config.rootUri matches
        Some(rest) => format!("{}{rest}", expand_replacement(root_uri, rest)),
        None => pattern.to_string(),
    };
    // "relative patterns" are "**/${pattern}"
    if !p.is_empty() && !p.starts_with('/') {
        p.insert_str(0, "**/");
    }
    // "/*" patterns are really "/**"
    if p.ends_with("/*") {
        p.push('*');
    }
    p
}

/// `rootUri + '/'` as a `String.prototype.replace` replacement for the leading `/`:
/// `$$`, `$&`, `` $` `` and `$'` are special even in a URI, where `$` stays raw.
fn expand_replacement(root_uri: &str, rest: &str) -> String {
    let mut out = String::with_capacity(root_uri.len() + 1);
    let mut it = root_uri.chars().peekable();
    while let Some(c) = it.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match it.peek() {
            Some('$') => {
                it.next();
                out.push('$');
            }
            Some('&') => {
                it.next();
                out.push('/');
            }
            Some('`') => {
                it.next();
            }
            Some('\'') => {
                it.next();
                out.push_str(rest);
            }
            _ => out.push('$'),
        }
    }
    out.push('/');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites() {
        assert_eq!(rewrite_pattern("/a/*", "file:///r"), "**/file:///r/a/**");
        assert_eq!(rewrite_pattern("a/*.js", "file:///r"), "**/a/*.js");
        assert_eq!(rewrite_pattern("", "file:///r"), "");
    }
}
