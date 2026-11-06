//! Upstream's text layout, an Ink card per file, and its JSON. `--output upstream` and
//! `--output json`. Both are parity surfaces: don't restyle them.

use lq_check::{Offense, Severity};
use lq_json::js::quote;
use lq_json::uri;
use std::collections::BTreeMap;

/// The width of a char in a terminal cell, as `string-width` counts it.
pub(crate) fn cell_width(c: char) -> usize {
    let u = c as u32;
    if c.is_control() || (0x300..=0x36F).contains(&u) || (0x200B..=0x200F).contains(&u) || (0xFE00..=0xFE0F).contains(&u) {
        return 0;
    }
    let wide = (0x1100..=0x115F).contains(&u)
        || (0x2E80..=0x303E).contains(&u)
        || (0x3041..=0x33FF).contains(&u)
        || (0x3400..=0x4DBF).contains(&u)
        || (0x4E00..=0x9FFF).contains(&u)
        || (0xA000..=0xA4CF).contains(&u)
        || (0xAC00..=0xD7A3).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFE30..=0xFE6F).contains(&u)
        || (0xFF00..=0xFF60).contains(&u)
        || (0xFFE0..=0xFFE6).contains(&u)
        || (0x1F300..=0x1F64F).contains(&u)
        || (0x1F900..=0x1F9FF).contains(&u)
        || (0x20000..=0x3FFFD).contains(&u);
    if wide {
        2
    } else {
        1
    }
}

pub(crate) fn width(s: &str) -> usize {
    s.chars().map(cell_width).sum()
}

/// `wrapAnsi(line, columns, { trim: false, hard: true })` for one line of plain text.
pub(crate) fn wrap_line(line: &str, columns: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    let words: Vec<&str> = line.split(' ').collect();
    let mut rows = vec![String::new()];
    let hard_wrap = |rows: &mut Vec<String>, word: &str| {
        let mut visible = width(rows.last().unwrap());
        for c in word.chars() {
            let w = cell_width(c);
            if visible + w > columns && visible > 0 {
                rows.push(String::new());
                visible = 0;
            }
            rows.last_mut().unwrap().push(c);
            visible += w;
            if visible == columns {
                rows.push(String::new());
                visible = 0;
            }
        }
        if rows.len() > 1 && rows.last().unwrap().is_empty() {
            rows.pop();
        }
    };
    for (index, word) in words.iter().enumerate() {
        let mut row_length = width(rows.last().unwrap());
        if index != 0 {
            if row_length >= columns {
                rows.push(String::new());
                row_length = 0;
            }
            if row_length > 0 {
                rows.last_mut().unwrap().push(' ');
                row_length += 1;
            }
        }
        let len = width(word);
        if len > columns {
            let remaining = columns - row_length;
            let this_line = 1 + (len - remaining - 1) / columns;
            let next_line = (len - 1) / columns;
            if next_line < this_line {
                rows.push(String::new());
            }
            hard_wrap(&mut rows, word);
            continue;
        }
        if row_length + len > columns && row_length > 0 {
            rows.push(String::new());
        }
        rows.last_mut().unwrap().push_str(word);
    }
    rows
}

/// An Ink `Alert` card, as the CLI draws it without color at width 80.
pub(crate) fn card(kind: &str, sections: &[String]) -> String {
    const W: usize = 80;
    let mut out = String::new();
    out.push_str("╭─ ");
    out.push_str(kind);
    out.push(' ');
    for _ in 0..W - 5 - kind.len() {
        out.push('─');
    }
    out.push_str("╮\n");
    let row = |out: &mut String, text: &str| {
        out.push_str("│  ");
        out.push_str(text);
        for _ in width(text)..W - 6 {
            out.push(' ');
        }
        out.push_str("  │\n");
    };
    row(&mut out, "");
    for (i, section) in sections.iter().enumerate() {
        if i > 0 {
            row(&mut out, "");
        }
        for line in section.split('\n') {
            for wrapped in wrap_line(line, W - 6) {
                row(&mut out, &wrapped);
            }
        }
    }
    row(&mut out, "");
    out.push('╰');
    for _ in 0..W - 2 {
        out.push('─');
    }
    out.push_str("╯\n");
    out
}

fn tag(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "[error]:",
        Severity::Warning => "[warning]:",
        Severity::Info => "[info]:",
    }
}

/// `Ne(source, startLine, endLine)`.
fn code_lines(source: &str, start: u32, end: u32) -> String {
    let lines: Vec<&str> = source.split('\n').collect();
    let (s, e) = (start as usize, (end as usize + 1).min(lines.len()));
    let slice = lines.get(s..e).unwrap_or(&[]);
    let single = slice.len() == 1;
    slice
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let text = if single { lq_syntax::js::trim(l) } else { l };
            format!("{}  {text}", s + i + 1)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn grouped(offenses: &[Offense]) -> Vec<(String, Vec<&Offense>)> {
    // `H`: by fsPath in first-offense order, each sorted (stably) by severity.
    let mut groups: Vec<(String, Vec<&Offense>)> = Vec::new();
    let mut index = BTreeMap::new();
    for o in offenses {
        let path = uri::to_native(&uri::fs_path(&o.uri));
        let i = *index.entry(path.clone()).or_insert_with(|| {
            groups.push((path, Vec::new()));
            groups.len() - 1
        });
        groups[i].1.push(o);
    }
    for (_, list) in &mut groups {
        list.sort_by_key(|o| o.severity);
    }
    groups
}

pub(crate) fn counts(list: &[&Offense]) -> (usize, usize, usize) {
    let n = |s| list.iter().filter(|o| o.severity == s).count();
    (n(Severity::Error), n(Severity::Warning), n(Severity::Info))
}

pub(crate) fn render_text(root: &str, groups: &[(String, Vec<&Offense>)], sources: &[(String, String)], inspected: usize, total: usize) -> String {
    let source_of: BTreeMap<&str, &str> = sources.iter().map(|(u, s)| (u.as_str(), s.as_str())).collect();
    let mut out = String::from("\n");
    let mut sorted: Vec<&(String, Vec<&Offense>)> = groups.iter().collect();
    sorted.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
    let root = uri::to_native(root);
    for (path, list) in sorted {
        // `c.replace(path, '').slice(1)`.
        let headline = match path.find(&root) {
            Some(i) => format!("{}{}", &path[..i], &path[i + root.len()..]),
            None => path.clone(),
        };
        let headline: String = headline.chars().skip(1).collect();
        let mut body = String::new();
        for (i, o) in list.iter().enumerate() {
            if i > 0 {
                body.push_str("\n\n");
            }
            body.push('\n');
            body.push_str(tag(o.severity));
            body.push(' ');
            body.push_str(o.check);
            body.push('\n');
            body.push_str(&o.message);
            if let Some(source) = source_of.get(o.uri.as_str()) {
                body.push_str("\n\n");
                body.push_str(&code_lines(source, o.start.line, o.end.line));
            }
        }
        out.push_str(&card("info", &[headline, body]));
        out.push('\n');
    }
    let mut lines = vec![format!("{inspected} files inspected")];
    if total == 0 {
        lines[0].push_str(" with no offenses found.");
    } else {
        lines[0].push_str(&format!(" with {total} total offenses found across {} files.", groups.len()));
        let all: Vec<&Offense> = groups.iter().flat_map(|(_, l)| l.iter().copied()).collect();
        let (e, w, i) = counts(&all);
        if e > 0 {
            lines.push(format!("{e} errors."));
        }
        if w > 0 {
            lines.push(format!("{w} warnings."));
        }
        if i > 0 {
            lines.push(format!("{i} info issues."));
        }
    }
    let kind = if total > 0 { "info" } else { "success" };
    out.push_str(&card(kind, &["Theme Check Summary.".to_string(), lines.join("\n")]));
    out.push('\n');
    out
}

/// `J(result)`: the `ThemeCheckResult` JSON. Rows and columns are one-based.
pub(crate) fn render_json(valid: bool, groups: &[(String, Vec<&Offense>)]) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("{\"valid\":");
    out.push_str(if valid { "true" } else { "false" });
    out.push_str(",\"issues\":[");
    let mut first = true;
    let (mut errors, mut warnings, mut infos) = (0, 0, 0);
    for (path, list) in groups {
        let (e, w, i) = counts(list);
        errors += e;
        warnings += w;
        infos += i;
        let file_path = quote(path);
        for o in list {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&format!(
                "{{\"filePath\":{file_path},\"check\":{},\"severity\":\"{}\",\"startRow\":{},\"startColumn\":{},\"endRow\":{},\"endColumn\":{},\"message\":{}}}",
                quote(o.check),
                o.severity.as_str(),
                o.start.line + 1,
                o.start.character + 1,
                o.end.line + 1,
                o.end.character + 1,
                quote(&o.message),
            ));
        }
    }
    out.push_str(&format!("],\"errorCount\":{errors},\"warningCount\":{warnings},\"infoCount\":{infos}}}"));
    out
}
