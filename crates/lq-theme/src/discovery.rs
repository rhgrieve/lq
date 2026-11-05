//! The file listing of theme-check-node's `getTheme`: `glob` 8.1.0 over
//! `<root>/**/*.{liquid,json}` with `{ absolute: true }`, sorted the way glob
//! sorts, then filtered through `isIgnored` with the absolute path.
//!
//! Semantics reproduced from glob's `_process*`/`_readdir*`:
//! - Entries starting with `.` are neither matched by `*.ext` nor descended into.
//! - A symlink to a directory is read one level (`link/*.liquid` matches) but not
//!   recursed into; the root itself may be a symlink and is walked in full.
//! - A symlink to a file matches on its name, broken or not. So does a *directory*
//!   called `x.json`. Upstream drops those when `readFile` fails in
//!   `toSourceCode`, so callers must also treat an unreadable path as absent.
//!
//! Deviations:
//! - A root with glob magic (`[x]`, `{a,b}`, `*`, `?`, `@(x)`...) is walked through
//!   this crate's minimatch 10 segments instead of glob's nested minimatch 5.1.9.
//!   A root segment that is exactly `**` is treated as a plain directory name.
//! - Upstream rejects on an unusual `readdir` error such as `EACCES`; here that
//!   directory is skipped.
//! - Sort ties (strings ICU compares equal, such as ones differing only in control
//!   characters) keep this walk's order; upstream's follow its async traversal.

use crate::config::Config;
use crate::ignore::is_ignored;
use crate::minimatch::{Minimatch, SegRef};
use lq_json::uri;
use std::cmp::Ordering;
use std::fs;

/// The absolute paths of the theme's files, in upstream's order.
pub fn get_theme_files(config: &Config) -> Vec<String> {
    let root = uri::posix_normalize(&uri::fs_path(&config.root_uri));
    let root = if root.len() > 1 { root.trim_end_matches('/').to_string() } else { root };
    let prune = prune_matchers(config);
    let mut found = Vec::new();
    if root.contains(['*', '?', '[', ']', '{', '}', '(', ')', '\\', '!', '+', '@']) {
        walk_magic_root(&root, &prune, &mut found);
    } else {
        walk(&root, false, &prune, &mut found);
    }
    let mut paths = sort_paths(found);
    paths.retain(|p| !is_ignored(p, config, None));
    paths
}

/// `*.liquid` / `*.json` as compiled by glob's nested minimatch:
/// `/^(?!\.)(?=.)[^/]*?\.liquid$/`.
fn matches_theme_file(name: &str) -> bool {
    !name.starts_with(['.', '\n', '\r', '\u{2028}', '\u{2029}'])
        && (name.ends_with(".liquid") || name.ends_with(".json"))
}

fn join(dir: &str, name: &str) -> String {
    let mut p = String::with_capacity(dir.len() + name.len() + 1);
    p.push_str(dir);
    if !dir.ends_with('/') {
        p.push('/');
    }
    p.push_str(name);
    p
}

/// The global ignore patterns that end in `/**`, which can prune the walk: a theme
/// usually has `node_modules/**`, and that tree can dwarf the theme.
///
/// If such a pattern matches `<dir>/<probe>`, its last globstar consumed the probe,
/// so it also matches every `<dir>/<non-dot segments>`, the only paths the walk could
/// find below `dir`. Upstream lists those files and filters them; the result is equal.
fn prune_matchers(config: &Config) -> Vec<&Minimatch> {
    let patterns = config.ignore.iter().flatten();
    patterns
        .zip(config.global_ignore_matchers())
        .filter(|(p, _)| p.ends_with("/**") || p.ends_with("/*"))
        .map(|(_, m)| m)
        .collect()
}

fn is_pruned(dir: &str, prune: &[&Minimatch]) -> bool {
    if prune.is_empty() {
        return false;
    }
    let probe = format!("{dir}/\u{1}lq-probe");
    prune.iter().any(|m| m.matches(&probe))
}

/// `dir/**/*.{liquid,json}`, where `dir` was reached without leaving a symlink.
fn walk(dir: &str, one_level: bool, prune: &[&Minimatch], out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut subdirs: Vec<(String, bool)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches_theme_file(&name) {
            out.push(join(dir, &name));
        }
        if one_level || name.starts_with('.') {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() || ft.is_symlink() {
            let sub = join(dir, &name);
            if !is_pruned(&sub, prune) {
                subdirs.push((sub, ft.is_symlink()));
            }
        }
    }
    for (sub, is_link) in subdirs {
        walk(&sub, is_link, prune, out);
    }
}

fn walk_magic_root(root: &str, prune: &[&Minimatch], out: &mut Vec<String>) {
    // The glob's own tail is split off: only the root part can be magic.
    let mm = Minimatch::new(root);
    if mm.error().is_some() {
        return;
    }
    for segs in mm.segments() {
        let mut dirs: Vec<String> = vec![String::new()];
        for (i, seg) in segs.iter().enumerate() {
            let mut next = Vec::new();
            for d in &dirs {
                match seg {
                    SegRef::Lit(l) if i == 0 && l.is_empty() => next.push("/".to_string()),
                    SegRef::Lit(l) => next.push(join(d, l)),
                    SegRef::Globstar => next.push(join(d, "**")),
                    SegRef::Magic(m) => {
                        let Ok(rd) = fs::read_dir(d) else { continue };
                        for entry in rd.flatten() {
                            let name = entry.file_name();
                            let name = name.to_string_lossy();
                            if !name.starts_with('.') && m.test(&name) {
                                next.push(join(d, &name));
                            }
                        }
                    }
                }
            }
            dirs = next;
        }
        for d in dirs {
            walk(&d, false, prune, out);
        }
    }
    out.sort();
    out.dedup();
}

/// `paths.sort((a, b) => a.localeCompare(b, 'en'))`, stable.
pub fn sort_paths(paths: Vec<String>) -> Vec<String> {
    let mut paths = paths;
    paths.sort_by_cached_key(|p| sort_key(p));
    paths
}

/// `a.localeCompare(b, 'en')`.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    sort_key(a).cmp(&sort_key(b))
}

// ---------------------------------------------------------------------------
// ICU root collation (`en` has no tailoring): non-ignorable punctuation, tertiary
// strength, no numeric ordering, compared level by level.
//
// Checked against `localeCompare` for U+0000-U+017F and stacked combining marks
// (the harness's [sort] line), apart from: U+0363-U+036F, which have primary weights
// here treated as ignorable, and the `l` + U+00B7 contraction (only the precomposed
// U+0140 is modelled). Above that it is an approximation: symbols and
// punctuation in U+2000-U+2BFF and emoji sort before digits, other characters
// after Latin by lowercased code point with uppercase as a tertiary difference.
// That gets scripts right relative to Latin, but not all orderings between
// scripts or within CJK, and contractions and expansions are not modelled there.
// ---------------------------------------------------------------------------

include!("collation_table.rs");

/// `primary, secondary, tertiary`, with 0 meaning "ignored at that level".
type Ce = (u32, u16, u16);

const COMMON_SEC: u16 = 1;
const COMMON_TER: u16 = 1;
const UPPER_TER: u16 = 2;

const OTHER_BASE: u32 = 0x20_0000;

fn push_ces(c: char, out: &mut Vec<Ce>) {
    let cp = c as u32;
    if let Some(ces) = LATIN_CES.get(cp as usize) {
        out.extend_from_slice(ces);
        return;
    }
    if let Some(sec) = cp.checked_sub(0x300).and_then(|i| MARK_SECONDARY.get(i as usize)) {
        if *sec != 0 {
            out.push((0, *sec, COMMON_TER));
        }
        return;
    }
    let symbol = (0x2000..0x2C00).contains(&cp) || (0x1F000..0x20000).contains(&cp);
    if symbol {
        out.push((SYMBOL_PRIMARY, 0, 0));
        out.push((cp, COMMON_SEC, COMMON_TER));
        return;
    }
    let lower = c.to_lowercase().next().unwrap_or(c);
    let ter = if lower != c { UPPER_TER } else { COMMON_TER };
    out.push((OTHER_BASE + lower as u32, COMMON_SEC, ter));
}

fn sort_key(s: &str) -> Vec<u32> {
    let mut ces: Vec<Ce> = Vec::with_capacity(s.len());
    for c in s.chars() {
        push_ces(c, &mut ces);
    }
    // Canonical ordering: a run of combining marks sorts by combining class.
    if !s.is_ascii() {
        let mut i = 0;
        while i < ces.len() {
            if ces[i].0 != 0 {
                i += 1;
                continue;
            }
            let start = i;
            while i < ces.len() && ces[i].0 == 0 {
                i += 1;
            }
            ces[start..i].sort_by_key(|c| SECONDARY_CCC.get(usize::from(c.1)).copied().unwrap_or(6));
        }
    }
    let mut key = Vec::with_capacity(ces.len() * 3 + 2);
    key.extend(ces.iter().filter(|c| c.0 != 0).map(|c| c.0));
    key.push(0);
    key.extend(ces.iter().filter(|c| c.1 != 0).map(|c| u32::from(c.1)));
    key.push(0);
    key.extend(ces.iter().filter(|c| c.2 != 0).map(|c| u32::from(c.2)));
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_order() {
        let mut v: Vec<String> = ["b", "A", "a", "B", "_", "-", "1", "a/b", "a-b", "a b", "a.b"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        v = sort_paths(v);
        assert_eq!(v, ["_", "-", "1", "a", "A", "a b", "a-b", "a.b", "a/b", "b", "B"]);
    }

    #[test]
    fn theme_file_names() {
        assert!(matches_theme_file("a.liquid"));
        assert!(matches_theme_file("a.b.json"));
        assert!(!matches_theme_file(".a.json"));
        assert!(!matches_theme_file(".json"));
        assert!(!matches_theme_file("a.JSON"));
        assert!(!matches_theme_file("\nx.json"));
    }
}
