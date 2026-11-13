//! Summaries of a profile tree (lq's estimate or a real Speedscope file), in text and JSON, and
//! the difference between two of them.

use crate::speedscope::Tree;
use std::collections::HashMap;
use std::fmt::Write as _;

#[derive(Clone, Debug, Default)]
pub struct Row {
    pub key: String,
    /// Where it is, as `file:line`, when it's a single place.
    pub location: String,
    pub calls: u32,
    pub self_us: f64,
    pub total_us: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub total_us: f64,
    pub nodes: usize,
    pub sections: Vec<Row>,
    pub snippets: Vec<Row>,
    pub files: Vec<Row>,
    pub lines: Vec<Row>,
}

/// `render 'card-product', product: p` → `card-product`.
fn snippet_name(name: &str) -> Option<String> {
    let rest = name.strip_prefix("render ").or_else(|| name.strip_prefix("include "))?;
    let rest = rest.trim_start();
    let q = rest.chars().next()?;
    if q != '\'' && q != '"' {
        return None;
    }
    let end = rest[1..].find(q)? + 1;
    Some(rest[1..end].to_string())
}

fn section_name(name: &str) -> Option<String> {
    // lq's own section frames: `section <id> (<type>)`.
    let rest = name.strip_prefix("section ")?;
    if rest.starts_with('\'') || rest.starts_with('"') {
        return None; // `{% section 'x' %}` tags are grouped by the frame lq opens inside them
    }
    Some(rest.to_string())
}

pub fn summarize(tree: &Tree) -> Summary {
    #[derive(Default)]
    struct Acc {
        calls: u32,
        self_us: f64,
        total_us: f64,
        location: String,
    }
    let mut sections: HashMap<String, Acc> = HashMap::new();
    let mut snippets: HashMap<String, Acc> = HashMap::new();
    let mut files: HashMap<String, Acc> = HashMap::new();
    let mut lines: HashMap<(String, u32, String), Acc> = HashMap::new();
    // Outermost-only totals, so recursive or nested renders of the same snippet aren't counted twice.
    let mut open: HashMap<String, u32> = HashMap::new();
    let mut stack: Vec<(u32, bool, Option<String>)> = tree.roots.iter().rev().map(|&r| (r, false, None)).collect();
    while let Some((n, exiting, group)) = stack.pop() {
        let node = &tree.nodes[n as usize];
        let f = &tree.frames[node.frame as usize];
        if exiting {
            if let Some(g) = group {
                *open.get_mut(&g).unwrap() -= 1;
            }
            continue;
        }
        let file = f.file.to_string();
        let fa = files.entry(file.clone()).or_default();
        fa.self_us += node.self_us;
        fa.calls += 1;
        let la = lines.entry((file.clone(), f.line, f.name.clone())).or_default();
        la.calls += 1;
        la.self_us += node.self_us;
        la.total_us += node.total_us;
        let mut group = None;
        if let Some(s) = section_name(&f.name) {
            let a = sections.entry(s.clone()).or_default();
            a.calls += 1;
            a.self_us += node.self_us;
            a.total_us += node.total_us;
            a.location = file.clone();
        } else if let Some(s) = snippet_name(&f.name) {
            let key = format!("snippet:{s}");
            let depth = open.entry(key.clone()).or_insert(0);
            let a = snippets.entry(s).or_default();
            a.calls += 1;
            if *depth == 0 {
                a.total_us += node.total_us;
            }
            *depth += 1;
            group = Some(key);
        }
        stack.push((n, true, group));
        for &c in node.children.iter().rev() {
            stack.push((c, false, None));
        }
    }
    let rows = |m: HashMap<String, Acc>| -> Vec<Row> {
        let mut v: Vec<Row> = m
            .into_iter()
            .map(|(k, a)| Row { key: k, location: a.location, calls: a.calls, self_us: a.self_us, total_us: a.total_us })
            .collect();
        v.sort_by(|a, b| b.total_us.max(b.self_us).total_cmp(&a.total_us.max(a.self_us)).then(a.key.cmp(&b.key)));
        v
    };
    let mut line_rows: Vec<Row> = lines
        .into_iter()
        .map(|((file, line, name), a)| Row {
            key: name,
            location: format!("{file}:{line}"),
            calls: a.calls,
            self_us: a.self_us,
            total_us: a.total_us,
        })
        .collect();
    line_rows.sort_by(|a, b| b.self_us.total_cmp(&a.self_us).then(a.location.cmp(&b.location)));
    let mut file_rows = rows(files);
    file_rows.sort_by(|a, b| b.self_us.total_cmp(&a.self_us).then(a.key.cmp(&b.key)));
    Summary {
        total_us: tree.total_us(),
        nodes: tree.nodes.len(),
        sections: rows(sections),
        snippets: rows(snippets),
        files: file_rows,
        lines: line_rows,
    }
}

pub fn ms(us: f64) -> String {
    let m = us / 1000.0;
    if m >= 100.0 { format!("{m:.0}") } else if m >= 10.0 { format!("{m:.1}") } else { format!("{m:.2}") }
}

fn pct(part: f64, total: f64) -> String {
    if total <= 0.0 { "-".into() } else { format!("{:.0}%", 100.0 * part / total) }
}

fn one_line(s: &str, max: usize) -> String {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > max {
        s.chars().take(max - 1).collect::<String>() + "…"
    } else {
        s
    }
}

pub struct TextOptions {
    pub top: usize,
}

pub fn text(s: &Summary, opts: &TextOptions) -> String {
    let mut o = String::new();
    let t = s.total_us;
    if !s.sections.is_empty() {
        let _ = writeln!(o, "{:<40}{:>8} {:>6}", "Sections", "ms", "share");
        for r in s.sections.iter().take(opts.top) {
            let _ = writeln!(o, "  {:<38}{:>8} {:>6}", one_line(&r.key, 38), ms(r.total_us), pct(r.total_us, t));
        }
        o.push('\n');
    }
    if !s.snippets.is_empty() {
        let _ = writeln!(o, "{:<40}{:>8} {:>6} {:>9}", "Snippets", "ms", "share", "renders");
        for r in s.snippets.iter().take(opts.top) {
            let _ = writeln!(o, "  {:<38}{:>8} {:>6} {:>9}", one_line(&r.key, 38), ms(r.total_us), pct(r.total_us, t), r.calls);
        }
        o.push('\n');
    }
    let _ = writeln!(o, "{:<10}{:>6}  Slowest lines (own time)", "  ms", "calls");
    for r in s.lines.iter().take(opts.top) {
        let _ = writeln!(o, "  {:>8} {:>6}  {}  {}", ms(r.self_us), r.calls, r.location, one_line(&r.key, 60));
    }
    o
}

pub fn json(s: &Summary) -> serde_json::Value {
    let rows = |v: &[Row], n: usize| -> Vec<serde_json::Value> {
        v.iter()
            .take(n)
            .map(|r| serde_json::json!({
                "name": r.key, "location": r.location, "calls": r.calls,
                "self_ms": (r.self_us / 10.0).round() / 100.0, "total_ms": (r.total_us / 10.0).round() / 100.0,
            }))
            .collect()
    };
    serde_json::json!({
        "total_ms": (s.total_us / 10.0).round() / 100.0,
        "nodes": s.nodes,
        "sections": rows(&s.sections, usize::MAX),
        "snippets": rows(&s.snippets, usize::MAX),
        "files": rows(&s.files, usize::MAX),
        "lines": rows(&s.lines, 200),
    })
}

/// The change from `base` to `head` by section, snippet and line, largest first.
pub fn diff_text(base: &Summary, head: &Summary, top: usize) -> String {
    let mut o = String::new();
    let d = head.total_us - base.total_us;
    let _ = writeln!(
        o,
        "Estimated render time: {} ms → {} ms ({}{} ms, {}{:.1}%)\n",
        ms(base.total_us),
        ms(head.total_us),
        if d >= 0.0 { "+" } else { "-" },
        ms(d.abs()),
        if d >= 0.0 { "+" } else { "-" },
        if base.total_us > 0.0 { 100.0 * d.abs() / base.total_us } else { 0.0 },
    );
    let section = |title: &str, a: &[Row], b: &[Row], own: bool, o: &mut String| {
        let val = |r: &Row| if own { r.self_us } else { r.total_us };
        // Lines match on file and code, not line number, so an edit above doesn't shift every row.
        let file = |r: &Row| r.location.rsplit_once(':').map_or(r.location.clone(), |(f, _)| f.to_string());
        let key = |r: &Row| if own { format!("{}\u{0}{}", file(r), r.key) } else { r.key.clone() };
        let mut m: HashMap<String, (f64, f64, u32, u32)> = HashMap::new();
        let mut shown: HashMap<String, String> = HashMap::new();
        for r in a {
            let e = m.entry(key(r)).or_default();
            e.0 += val(r);
            e.2 += r.calls;
            shown.entry(key(r)).or_insert_with(|| if own { format!("{}  {}", r.location, r.key) } else { r.key.clone() });
        }
        for r in b {
            let e = m.entry(key(r)).or_default();
            e.1 += val(r);
            e.3 += r.calls;
            shown.insert(key(r), if own { format!("{}  {}", r.location, r.key) } else { r.key.clone() });
        }
        let mut v: Vec<_> = m.into_iter().filter(|(_, x)| (x.1 - x.0).abs() >= 1.0).collect();
        v.sort_by(|x, y| (y.1 .1 - y.1 .0).abs().total_cmp(&(x.1 .1 - x.1 .0).abs()).then(x.0.cmp(&y.0)));
        if v.is_empty() {
            return;
        }
        let _ = writeln!(o, "{title}");
        for (k, (x, y, cx, cy)) in v.into_iter().take(top) {
            let k = shown.get(&k).cloned().unwrap_or(k);
            let delta = y - x;
            let calls = if cx != cy { format!("  ({cx} → {cy} calls)") } else { String::new() };
            let _ = writeln!(o, "  {}{:>8} ms  {}{calls}", if delta >= 0.0 { "+" } else { "-" }, ms(delta.abs()), one_line(&k, 70));
        }
        o.push('\n');
    };
    section("Sections", &base.sections, &head.sections, false, &mut o);
    section("Snippets", &base.snippets, &head.snippets, false, &mut o);
    section("Lines (own time)", &base.lines, &head.lines, true, &mut o);
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_names() {
        assert_eq!(snippet_name("render 'card-product', product: p").as_deref(), Some("card-product"));
        assert_eq!(snippet_name("render \"x\"").as_deref(), Some("x"));
        assert_eq!(snippet_name("render block"), None);
        assert_eq!(section_name("section main (main-product)").as_deref(), Some("main (main-product)"));
    }
}
