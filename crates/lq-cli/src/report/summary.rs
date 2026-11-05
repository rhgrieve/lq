//! The end of the report: what was found, what can be done, and why the run passed or failed.

use super::rail::severity_code;
use super::term::{style, Term};
use super::{fail_threshold, Report};
use lq_check::Severity;
use std::collections::{BTreeMap, HashSet};

const BAR_WIDTH: usize = 16;

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn duration(d: std::time::Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1000.0 {
        format!("{ms:.0}ms")
    } else {
        format!("{:.2}s", ms / 1000.0)
    }
}

pub fn summary(report: &Report<'_>, term: &Term) -> String {
    let mut out = String::new();
    let total = report.offenses.len();
    let inspected = plural(report.inspected, "file", "files");
    let time = duration(report.elapsed);
    if total == 0 {
        out.push_str(&format!("{} {inspected} checked in {time}, no offenses\n", term.paint(style::SUCCESS, "✓")));
        return out;
    }
    let count = |s: Severity| report.offenses.iter().filter(|o| o.severity == s).count();
    let (errors, warnings, infos) = (count(Severity::Error), count(Severity::Warning), count(Severity::Info));
    let mut parts = Vec::new();
    if errors > 0 {
        parts.push(term.paint(style::ERROR, &format!("✖ {}", plural(errors, "error", "errors"))));
    }
    if warnings > 0 {
        parts.push(term.paint(style::WARNING, &format!("▲ {}", plural(warnings, "warning", "warnings"))));
    }
    if infos > 0 {
        parts.push(term.paint(style::INFO, &format!("● {infos} info")));
    }
    out.push_str(&parts.join("   "));
    out.push_str("\n\n");

    let mut by_check: BTreeMap<&str, (usize, Severity)> = BTreeMap::new();
    for o in &report.offenses {
        let entry = by_check.entry(o.check).or_insert((0, o.severity));
        entry.0 += 1;
        entry.1 = entry.1.min(o.severity);
    }
    let mut rows: Vec<(&str, usize, Severity)> = by_check.into_iter().map(|(c, (n, s))| (c, n, s)).collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)).then(a.0.cmp(b.0)));
    let max = rows.first().map_or(1, |r| r.1);
    let number = max.to_string().len();
    for (check, n, severity) in rows {
        let cells = (n * BAR_WIDTH).div_ceil(max);
        let bar = term.paint(severity_code(severity), &"█".repeat(cells));
        let gap = " ".repeat(BAR_WIDTH - cells);
        out.push_str(&format!("  {n:>number$} {bar}{gap} {check}\n"));
    }
    out.push('\n');

    let fixable = report.offenses.iter().filter(|o| report.can_auto_correct(o)).count();
    let by_hand = report.offenses.iter().filter(|o| o.fix.is_some()).count() - fixable;
    let suggested = report.offenses.iter().filter(|o| o.suggest.as_ref().is_some_and(|s| !s.is_empty())).count();
    if report.applied == Some((0, 0)) {
        out.push_str(&format!("{} --auto-correct changed nothing\n", term.paint(style::DIM, "→")));
    } else if let Some((fixes, files)) = report.applied {
        let text = format!("Applied {} in {}; the report above is from before they ran", plural(fixes, "fix", "fixes"), plural(files, "file", "files"));
        out.push_str(&format!("{} {text}\n", term.paint(style::SUCCESS, "✓")));
    } else if fixable > 0 {
        let text = format!("{} can be fixed with lq check --auto-correct", plural(fixable, "offense", "offenses"));
        out.push_str(&format!("{} {text}\n", term.paint(style::GREEN, "→")));
    }
    if by_hand > 0 {
        let text = format!("{} a fix --auto-correct can't apply, shown as \"fix\"", plural(by_hand, "offense has", "offenses have"));
        out.push_str(&format!("{} {text}\n", term.paint(style::WARNING, "→")));
    }
    if suggested > 0 {
        let text = format!("{} a suggested change, shown as \"try\"", plural(suggested, "offense has", "offenses have"));
        out.push_str(&format!("{} {text}\n", term.paint(style::GREEN, "→")));
    }

    let files: HashSet<&str> = report.offenses.iter().map(|o| o.uri.as_str()).collect();
    let found = plural(total, "offense", "offenses");
    out.push_str(&format!("{found} in {} of {inspected} checked in {time}\n", files.len()));
    out.push_str(&verdict(report, term, errors, warnings, infos));
    out
}

/// Says why the exit code is what it is.
fn verdict(report: &Report<'_>, term: &Term, errors: usize, warnings: usize, infos: usize) -> String {
    let level = report.fail_level;
    let threshold = fail_threshold(level).filter(|_| !report.valid);
    let Some(threshold) = threshold else {
        return format!("{} nothing at fail level {level}\n", term.paint(style::SUCCESS, "passed:"));
    };
    let counted = [(Severity::Error, errors), (Severity::Warning, warnings), (Severity::Info, infos)];
    let failing: usize = counted.iter().filter(|(s, _)| *s <= threshold).map(|(_, n)| n).sum();
    let others: usize = counted.iter().filter(|(s, _)| *s > threshold).map(|(_, n)| n).sum();
    let mut text = format!("{} at fail level {level}", plural(failing, "offense", "offenses"));
    match others {
        0 => {}
        1 => text.push_str("; the other one doesn't fail this run"),
        n => text.push_str(&format!("; the other {n} don't fail this run")),
    }
    format!("{} {text}\n", term.paint(style::ERROR, "failed:"))
}
