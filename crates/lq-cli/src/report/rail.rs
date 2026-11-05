//! The default report: each diagnostic stands alone, on a rail.

use super::diff::preview;
use super::fold::{fold, unfolded, Group};
use super::highlight::{highlight, Highlights};
use super::sanitize;
use super::snippet::{self, floor_boundary, LineIndex, Span};
use super::summary::summary;
use super::term::{style, Term};
use super::upstream::wrap_line;
use super::Report;
use lq_check::registry::REGISTRY;
use lq_check::{Offense, Severity};
use std::collections::HashMap;

/// Suggestions shown per diagnostic; `--output json-rich` has them all.
const MAX_SUGGESTIONS: usize = 3;
/// `note `, `fix  `: the key column and its space.
const KEY_WIDTH: usize = 5;

pub fn severity_code(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => style::ERROR,
        Severity::Warning => style::WARNING,
        Severity::Info => style::INFO,
    }
}

/// A file's derived data, kept since a file usually has several diagnostics.
struct FileCache<'s> {
    index: LineIndex,
    highlights: Option<Highlights>,
    source: &'s str,
}

/// A row under the snippet.
struct Item {
    key: &'static str,
    /// Styled lines, already fitting the width.
    lines: Vec<String>,
}

pub fn render(report: &Report<'_>, term: &Term) -> String {
    let groups = if report.fold { fold(&report.offenses) } else { unfolded(&report.offenses) };
    let mut cache: HashMap<&str, FileCache<'_>> = HashMap::new();
    let mut out = String::from("\n");
    for group in &groups {
        diagnostic(&mut out, report, term, group, &mut cache);
        out.push('\n');
    }
    out.push_str(&summary(report, term));
    out
}

fn diagnostic<'s>(out: &mut String, report: &Report<'s>, term: &Term, group: &Group<'_>, cache: &mut HashMap<&'s str, FileCache<'s>>) {
    let o = group.first;
    let code = severity_code(o.severity);
    out.push_str(&term.paint(code, o.severity.as_str()));
    out.push(' ');
    out.push_str(&term.paint(style::BOLD, o.check));
    out.push('\n');
    for line in o.message.split('\n') {
        for wrapped in wrap_line(&sanitize(line), term.width) {
            out.push_str(wrapped.trim_end());
            out.push('\n');
        }
    }

    let file = report.sources.get_key_value(o.uri.as_str()).map(|(uri, source)| {
        &*cache.entry(uri).or_insert_with(|| FileCache {
            index: LineIndex::new(source),
            highlights: term.color.then(|| highlight(source, uri.ends_with(".json"))),
            source,
        })
    });
    let span = Span { start: o.start_byte as usize, end: o.end_byte as usize, label: o.hints.label.as_deref(), code, file_level: o.hints.file_level };
    let snippet = file.and_then(|f| snippet::render(f.source, &f.index, &span, term, f.highlights.as_ref()));
    let gutter = snippet.as_ref().map_or(1, |s| s.gutter);
    let rail = " ".repeat(gutter + 1);
    let paint_rail = |s: &str| term.paint(style::RAIL, s);

    out.push_str(&format!("{rail}{} {}\n", paint_rail("╭─"), term.paint(style::CYAN, &report.location(o))));
    let items = items(report, term, group, gutter);
    if let Some(snippet) = &snippet {
        out.push_str(&format!("{rail}{}\n", paint_rail("│")));
        for row in &snippet.rows {
            match row.number {
                Some(n) => out.push_str(&format!("{} {} {}\n", paint_rail(&format!("{n:>gutter$}")), paint_rail("│"), row.text)),
                None => out.push_str(&format!("{rail}{} {}\n", paint_rail("·"), row.text)),
            }
        }
    }
    out.push_str(&format!("{rail}{}\n", paint_rail("│")));
    for (i, item) in items.iter().enumerate() {
        let last = i + 1 == items.len();
        for (j, line) in item.lines.iter().enumerate() {
            if j == 0 {
                let joint = paint_rail(if last { "╰" } else { "├" });
                let key = term.paint(style::BOLD, &format!("{:<4}", item.key));
                out.push_str(&format!("{rail}{joint} {key} {line}\n"));
            } else {
                let bar = if last { " ".to_string() } else { paint_rail("│") };
                let key = " ".repeat(KEY_WIDTH - 1);
                out.push_str(&format!("{rail}{bar} {key} {line}\n"));
            }
        }
    }
}

/// The width left for a row's text.
fn text_width(term: &Term, gutter: usize) -> usize {
    term.width.saturating_sub(gutter + 3 + KEY_WIDTH).max(20)
}

fn wrapped(text: &str, width: usize) -> Vec<String> {
    wrap_line(&sanitize(text), width).iter().map(|l| l.trim_end().to_string()).collect()
}

fn items(report: &Report<'_>, term: &Term, group: &Group<'_>, gutter: usize) -> Vec<Item> {
    let o = group.first;
    let width = text_width(term, gutter);
    let source = report.sources.get(o.uri.as_str()).copied();
    let mut items = Vec::new();

    for note in &o.hints.notes {
        let mut lines = wrapped(&note.message, width);
        if let Some(related) = &note.related {
            lines.push(term.paint(style::CYAN, &related_location(report, &related.uri, related.start)));
        }
        items.push(Item { key: "note", lines });
    }
    if let (Some(fix), Some(source)) = (&o.fix, source) {
        let mut lines = match report.unfixable.get(o.uri.as_str()) {
            None => vec![term.paint(style::BOLD, "lq check --auto-correct applies this")],
            Some(reason) => {
                let text = format!("--auto-correct can't change this file, so make this change by hand: {reason}");
                wrapped(&text, width)
            }
        };
        lines.extend(preview(source, fix, term, width));
        items.push(Item { key: "fix", lines });
    }
    if let Some(suggestions) = &o.suggest {
        for s in suggestions.iter().take(MAX_SUGGESTIONS) {
            let mut lines = wrapped(&s.message, width);
            if let Some(source) = source {
                lines.extend(preview(source, &s.fix, term, width));
            }
            items.push(Item { key: "try", lines });
        }
        if suggestions.len() > MAX_SUGGESTIONS {
            let more = format!("+{} more, in --output json-rich", suggestions.len() - MAX_SUGGESTIONS);
            items.push(Item { key: "try", lines: vec![term.paint(style::DIM, &more)] });
        }
    }
    if !group.rest.is_empty() {
        let n = group.rest.len();
        let places = if n == 1 { "place" } else { "places" };
        let mut lines = vec![format!("the same problem in {n} more {places}")];
        // Never wrapped: a location has to stay whole to be useful.
        lines.extend(group.rest.iter().map(|other| term.paint(style::CYAN, &report.location(other))));
        items.push(Item { key: "also", lines });
    }
    items.push(Item { key: "docs", lines: vec![docs_line(o, term)] });
    items
}

fn docs_line(o: &Offense, term: &Term) -> String {
    let url = REGISTRY.iter().find(|d| d.meta.code == o.check).and_then(|d| d.meta.url);
    let name = match url {
        Some(url) => term.link(url, o.check),
        None => o.check.to_string(),
    };
    format!("lq explain {name}")
}

fn related_location(report: &Report<'_>, uri: &str, start: u32) -> String {
    let path = report.display_path(uri);
    let Some(source) = report.sources.get(uri) else { return path };
    let (line, column) = line_column(source, start as usize);
    format!("{path}:{line}:{column}")
}

/// One-based, in the UTF-16 columns that an offense's own location uses.
fn line_column(source: &str, byte: usize) -> (usize, usize) {
    let before = &source[..floor_boundary(source, byte)];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.encode_utf16().count()) + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::line_column;

    #[test]
    fn locates_a_related_byte() {
        assert_eq!(line_column("abc", 0), (1, 1));
        assert_eq!(line_column("abc\ndef", 5), (2, 2));
        assert_eq!(line_column("日本\n😀x", 11), (2, 3));
        assert_eq!(line_column("abc", 99), (1, 4));
        assert_eq!(line_column("日", 1), (1, 1));
    }
}
