//! How `lq check` presents offenses. `upstream` is parity; everything else is `lq`'s own,
//! and only ever changes the presentation of the offenses it's given.

pub mod diff;
pub mod fold;
pub mod highlight;
pub mod machine;
pub mod rail;
pub mod snippet;
pub mod summary;
pub mod term;
pub mod upstream;

use lq_check::fixes::{fix_all, Fix};
use lq_check::{Offense, Severity};
use lq_json::uri;
use std::collections::BTreeMap;
use std::time::Duration;

/// What `lq check` was asked to print, and the run's outcome.
pub struct Report<'a> {
    pub root: &'a str,
    pub cwd: String,
    /// Sorted by file, then by position in the file.
    pub offenses: Vec<&'a Offense>,
    pub sources: BTreeMap<&'a str, &'a str>,
    pub inspected: usize,
    pub valid: bool,
    pub fail_level: &'a str,
    pub elapsed: Duration,
    pub fold: bool,
    /// Fixes and files `--auto-correct` changed.
    pub applied: Option<(usize, usize)>,
    /// Files whose JSON fixes `--auto-correct` can't apply, and why. See [`Report::check_fixes`].
    pub unfixable: BTreeMap<&'a str, String>,
}

impl<'a> Report<'a> {
    pub fn new(root: &'a str, offenses: &'a [Offense], sources: &'a [(String, String)]) -> Report<'a> {
        let mut sorted: Vec<(String, &Offense)> = offenses.iter().map(|o| (uri::fs_path(&o.uri), o)).collect();
        sorted.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()).then(a.1.start_byte.cmp(&b.1.start_byte)));
        Report {
            root,
            cwd: std::env::current_dir().map_or_else(|_| String::new(), |d| uri::posix_normalize(&uri::from_native(&d.to_string_lossy()))),
            offenses: sorted.into_iter().map(|(_, o)| o).collect(),
            sources: sources.iter().map(|(u, s)| (u.as_str(), s.as_str())).collect(),
            inspected: 0,
            valid: true,
            fail_level: "error",
            elapsed: Duration::ZERO,
            fold: true,
            applied: None,
            unfixable: BTreeMap::new(),
        }
    }

    /// Finds out which fixes can't be applied, so that the report doesn't promise them. Applies
    /// each file's fixes together, as `--auto-correct` does, since it's all or nothing per file:
    /// a comment header in a locale stops them, and so do edits that overlap.
    pub fn check_fixes(&mut self) {
        let mut by_file: BTreeMap<&str, Vec<&Fix>> = BTreeMap::new();
        for o in &self.offenses {
            if let Some(fix) = &o.fix {
                by_file.entry(o.uri.as_str()).or_default().push(fix);
            }
        }
        for (uri, fixes) in by_file {
            let Some((uri, source)) = self.sources.get_key_value(uri) else { continue };
            if let Err(reason) = fix_all(source, &fixes) {
                self.unfixable.insert(uri, reason);
            }
        }
    }

    /// Whether `--auto-correct` will apply this offense's fix.
    pub fn can_auto_correct(&self, o: &Offense) -> bool {
        o.fix.is_some() && !self.unfixable.contains_key(o.uri.as_str())
    }

    /// A path to print: from the current directory when it's under it, which is what a
    /// terminal or an editor can open, or else from the theme root.
    pub fn display_path(&self, file_uri: &str) -> String {
        let path = uri::fs_path(file_uri);
        for base in [self.cwd.as_str(), self.root] {
            if !base.is_empty() {
                if let Some(rest) = path.strip_prefix(base).and_then(|r| r.strip_prefix('/')) {
                    return sanitize(rest);
                }
            }
        }
        sanitize(&path)
    }

    /// `path:line:col`, one-based and in the columns `--output json` reports.
    pub fn location(&self, o: &Offense) -> String {
        format!("{}:{}:{}", self.display_path(&o.uri), o.start.line + 1, o.start.character + 1)
    }
}

/// `Ae(failLevel)`: the severity at or below which the run fails. `None` never fails.
pub fn fail_threshold(level: &str) -> Option<Severity> {
    match level {
        "error" => Some(Severity::Error),
        "suggestion" | "warning" => Some(Severity::Warning),
        "style" | "info" => Some(Severity::Info),
        _ => None,
    }
}

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
