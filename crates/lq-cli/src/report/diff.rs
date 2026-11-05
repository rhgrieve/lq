//! Previews of fixes and suggestions.

use super::sanitize;
use super::term::{style, Term};
use lq_check::fixes::{fix_all, Fix, JsonPatch};

const MAX_LINES: usize = 8;
const MAX_VALUE: usize = 60;

/// What a fix changes, as lines to print. Empty if there's nothing useful to show.
pub fn preview(source: &str, fix: &Fix, term: &Term, width: usize) -> Vec<String> {
    match fix {
        Fix::Edits(edits) => {
            let Some(start) = edits.iter().map(|e| e.start as usize).min() else { return Vec::new() };
            let end = edits.iter().map(|e| e.end as usize).max().unwrap_or(start);
            edit_lines(source, fix, start, end, term, width)
        }
        Fix::Json(patches) => json_lines(patches, term, width),
    }
}

fn edit_lines(source: &str, fix: &Fix, start: usize, end: usize, term: &Term, width: usize) -> Vec<String> {
    if start > source.len() || end > source.len() || !source.is_char_boundary(start) || !source.is_char_boundary(end) {
        return Vec::new();
    }
    let Ok(fixed) = fix_all(source, &[fix]) else { return Vec::new() };
    let region_start = source[..start].rfind('\n').map_or(0, |i| i + 1);
    let region_end = source[end..].find('\n').map_or(source.len(), |i| end + i);
    let tail = source.len() - region_end;
    let Some(new_end) = fixed.len().checked_sub(tail) else { return Vec::new() };
    if new_end < region_start || !fixed.is_char_boundary(new_end) || !fixed.is_char_boundary(region_start) {
        return Vec::new();
    }
    let old: Vec<&str> = source[region_start..region_end].split('\n').collect();
    let new: Vec<&str> = fixed[region_start..new_end].split('\n').collect();

    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
    let removed = &old[prefix..old.len() - suffix];
    // Removing everything on a line leaves only its indent: that's the line going away.
    let added: Vec<&str> = new[prefix..new.len() - suffix].iter().copied().filter(|l| !l.trim().is_empty()).collect();
    let indent = |l: &&str| l.len() - l.trim_start().len();
    let dedent = removed.iter().chain(&added).filter(|l| !l.trim().is_empty()).map(indent).min().unwrap_or(0);

    let mut out = Vec::new();
    let mut side = |lines: &[&str], sign: &str, code: &str| {
        for line in lines.iter().take(MAX_LINES) {
            let text = sanitize(line.trim_end_matches('\r').get(dedent.min(indent(line))..).unwrap_or(line));
            out.push(term.paint(code, &clip(&format!("{sign} {text}"), width)));
        }
        if lines.len() > MAX_LINES {
            out.push(term.paint(style::DIM, &format!("… {} more lines", lines.len() - MAX_LINES)));
        }
    };
    side(removed, "-", style::RED);
    side(&added, "+", style::GREEN);
    out
}

/// A JSON fix rewrites the file through its corrector, which can reformat it, so a text diff
/// could bury the change. The patches say what it is.
fn json_lines(patches: &[JsonPatch], term: &Term, width: usize) -> Vec<String> {
    let value_text = |v: &lq_json::Value| clip(&sanitize(&v.to_json()), MAX_VALUE);
    let mut out = Vec::new();
    let mut i = 0;
    while i < patches.len() {
        match (&patches[i], patches.get(i + 1)) {
            (JsonPatch::Remove { path }, Some(JsonPatch::Add { path: next, value })) if path == next => {
                let text = format!("~ {} = {}", sanitize(path), value_text(value));
                out.push(term.paint(style::CYAN, &clip(&text, width)));
                i += 2;
            }
            (JsonPatch::Remove { path }, _) => {
                out.push(term.paint(style::RED, &clip(&format!("- {}", sanitize(path)), width)));
                i += 1;
            }
            (JsonPatch::Add { path, value }, _) => {
                let text = format!("+ {} = {}", sanitize(path), value_text(value));
                out.push(term.paint(style::GREEN, &clip(&text, width)));
                i += 1;
            }
        }
    }
    out
}

fn clip(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let keep: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{keep}…")
}

