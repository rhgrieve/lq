//! `--only`, `--skip` and `--severity`: narrow what is reported, never what is found.
//!
//! Every check still runs, so a filtered report is exactly a subset of the full one. Like file
//! arguments, filters decide the exit code and what `--auto-correct` touches.

use lq_check::check_utils::{closest, did_you_mean};
use lq_check::registry::REGISTRY;
use lq_check::{Offense, Severity};
use std::collections::BTreeSet;

/// A selector from the command line and the checks it stands for.
struct Selector {
    text: String,
    codes: BTreeSet<&'static str>,
}

#[derive(Default)]
pub struct Filter {
    only: Vec<Selector>,
    skip: BTreeSet<&'static str>,
    severity: Option<Severity>,
}

/// `pattern` is lowercase. `*` anchors it at the ends it's used with, and without one it matches anywhere.
fn pattern_matches(pattern: &str, code: &str) -> bool {
    if !pattern.contains('*') {
        return code.contains(pattern);
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let Some(mut rest) = code.strip_prefix(parts[0]) else { return false };
    for part in &parts[1..parts.len() - 1] {
        let Some(at) = rest.find(part) else { return false };
        rest = &rest[at + part.len()..];
    }
    rest.ends_with(parts[parts.len() - 1])
}

/// The checks `text` selects. A full name or alias means just that check, so `ValidSchema` doesn't
/// also take `ValidSchemaName`, and any check can be picked on its own.
fn resolve(text: &str) -> BTreeSet<&'static str> {
    let exact = REGISTRY.iter().find(|d| {
        d.meta.code.eq_ignore_ascii_case(text) || d.meta.aliases.iter().any(|a| a.eq_ignore_ascii_case(text))
    });
    if let Some(def) = exact {
        return BTreeSet::from([def.meta.code]);
    }
    let pattern = text.to_ascii_lowercase();
    REGISTRY
        .iter()
        .map(|d| d.meta.code)
        .filter(|code| pattern_matches(&pattern, &code.to_ascii_lowercase()))
        .collect()
}

/// A selector that matches nothing is a mistake: taken quietly, a typo would read as a clean theme.
fn selector(flag: &str, text: &str) -> Result<Selector, String> {
    let codes = resolve(text);
    if !codes.is_empty() {
        return Ok(Selector { text: text.to_string(), codes });
    }
    let mut message = format!("[lq] error: no check matches {flag} {}", crate::report::sanitize(text));
    if let Some(found) = closest(text, REGISTRY.iter().map(|d| d.meta.code)) {
        message.push('\n');
        message.push_str(&did_you_mean(found));
    }
    Err(message)
}

impl Filter {
    pub fn new(only: &[String], skip: &[String], severity: Option<Severity>) -> Result<Filter, String> {
        let selectors = |flag, texts: &[String]| texts.iter().map(|t| selector(flag, t)).collect::<Result<Vec<_>, _>>();
        Ok(Filter {
            only: selectors("--only", only)?,
            skip: selectors("--skip", skip)?.into_iter().flat_map(|s| s.codes).collect(),
            severity,
        })
    }

    pub fn allows(&self, code: &str, severity: Severity) -> bool {
        (self.only.is_empty() || self.only.iter().any(|s| s.codes.contains(code)))
            && !self.skip.contains(code)
            && self.severity.is_none_or(|least| severity <= least)
    }

    fn flags(&self) -> String {
        let flags = [
            (!self.only.is_empty(), "--only"),
            (!self.skip.is_empty(), "--skip"),
            (self.severity.is_some(), "--severity"),
        ];
        let on: Vec<&str> = flags.into_iter().filter(|(on, _)| *on).map(|(_, name)| name).collect();
        on.join(" and ")
    }

    /// Drops what the filter leaves out. The note says how much that was, so that a short report
    /// doesn't read as a healthy theme.
    pub fn apply(&self, offenses: &mut Vec<Offense>) -> Option<String> {
        let total = offenses.len();
        let mut errors = 0;
        offenses.retain(|o| {
            let keep = self.allows(o.check, o.severity);
            errors += usize::from(!keep && o.severity == Severity::Error);
            keep
        });
        let hidden = total - offenses.len();
        if hidden == 0 {
            return None;
        }
        let errors = match errors {
            0 => String::new(),
            1 => ", including 1 error".to_string(),
            n => format!(", including {n} errors"),
        };
        Some(format!("[lq] note: {hidden} of {total} offenses not shown because of {}{errors}.", self.flags()))
    }

    /// `--only` can't find what didn't run. One note for each selector whose checks are all off.
    pub fn turned_off_notes(&self, enabled: &[&str]) -> Vec<String> {
        self.only
            .iter()
            .filter(|s| !s.codes.iter().any(|code| enabled.contains(code)))
            .map(|s| {
                let names: Vec<&str> = s.codes.iter().copied().collect();
                let verb = if names.len() == 1 { "is" } else { "are" };
                format!(
                    "[lq] note: {} {verb} turned off in the config, so --only {} can't find anything. Turn it on in .theme-check.yml, or try --config theme-check:all.",
                    names.join(", "),
                    crate::report::sanitize(&s.text),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(text: &str) -> Vec<&'static str> {
        resolve(text).into_iter().collect()
    }

    #[test]
    fn a_full_name_is_only_that_check_in_any_case() {
        assert_eq!(codes("ValidSchema"), ["ValidSchema"]);
        assert_eq!(codes("orphanedsnippet"), ["OrphanedSnippet"]);
    }

    #[test]
    fn an_alias_selects_its_check() {
        assert_eq!(codes("AssetSizeCSSStylesheetTag"), ["AssetSizeCSS"]);
    }

    #[test]
    fn part_of_a_name_selects_every_check_containing_it() {
        assert_eq!(codes("orphan"), ["OrphanedSnippet"]);
        let codes = codes("assetsize");
        assert!(codes.contains(&"AssetSizeCSS") && codes.contains(&"AssetSizeJavaScript"), "{codes:?}");
        assert!(!codes.contains(&"RemoteAsset"), "{codes:?}");
    }

    #[test]
    fn a_star_anchors_the_pattern() {
        assert!(codes("Asset*").contains(&"AssetPreload"));
        assert!(!codes("Asset*").contains(&"RemoteAsset"));
        assert!(codes("*Asset").contains(&"RemoteAsset"));
        assert!(!codes("*Asset").contains(&"AssetPreload"));
        assert_eq!(codes("orph*snip*"), ["OrphanedSnippet"]);
        assert_eq!(codes("orphanedsnippet*x"), Vec::<&str>::new());
    }

    #[test]
    fn a_star_does_not_match_the_same_text_twice() {
        assert!(!pattern_matches("abc*bcd", "abcd"));
        assert!(pattern_matches("abc*bcd", "abcbcd"));
    }

    #[test]
    fn a_typo_is_an_error_with_a_suggestion() {
        let e = Filter::new(&["OrphanedSnipet".to_string()], &[], None).err().unwrap();
        assert!(e.contains("no check matches --only OrphanedSnipet"), "{e}");
        assert!(e.contains("did you mean 'OrphanedSnippet'?"), "{e}");
    }

    #[test]
    fn skip_beats_only_and_severity_is_a_minimum() {
        let filter = Filter::new(&["asset".to_string()], &["AssetPreload".to_string()], Some(Severity::Warning)).unwrap();
        assert!(filter.allows("RemoteAsset", Severity::Error));
        assert!(filter.allows("RemoteAsset", Severity::Warning));
        assert!(!filter.allows("RemoteAsset", Severity::Info));
        assert!(!filter.allows("AssetPreload", Severity::Error));
        assert!(!filter.allows("UnusedAssign", Severity::Error));
    }

    #[test]
    fn no_filter_allows_everything() {
        assert!(Filter::default().allows("UnusedAssign", Severity::Info));
    }
}
