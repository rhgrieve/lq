//! Summaries of a profile tree (lq's estimate or a real Speedscope file), in text and JSON, and
//! the difference between two of them.
//!
//! Both kinds of tree have Shopify's shape, so one summary serves both: templates are the
//! `liquid_template` frames, grouped by file, and lines are frames with a line, with the time of
//! their filters and wrappers attributed to them.

use crate::speedscope::{attribution, Tree};
use std::collections::HashMap;
use std::fmt::Write as _;

#[derive(Clone, Debug, Default)]
pub struct Row {
    /// A template name (`sections/header`), or a line's code.
    pub key: String,
    /// `file:line` for lines.
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
    pub blocks: Vec<Row>,
    pub snippets: Vec<Row>,
    pub apps: Vec<Row>,
    pub lines: Vec<Row>,
}

/// Source text of `file:line`, for showing lines whose frames only name the tag class.
pub type SourceLine<'a> = &'a dyn Fn(&str, u32) -> Option<String>;

fn app_name(file: &str) -> Option<String> {
    let rest = file.strip_prefix("shopify://apps/")?;
    let mut parts = rest.split('/');
    let app = parts.next()?;
    let _ = parts.next(); // "blocks"
    let block = parts.next().unwrap_or("");
    Some(if block.is_empty() { app.to_string() } else { format!("{app}/{block}") })
}

pub fn summarize(tree: &Tree, source: Option<SourceLine<'_>>) -> Summary {
    #[derive(Default)]
    struct Acc {
        calls: u32,
        self_us: f64,
        total_us: f64,
    }
    let keys = attribution(tree);
    let mut templates: HashMap<String, Acc> = HashMap::new();
    let mut lines: HashMap<(String, u32), (Acc, String)> = HashMap::new();
    // Templates count total time once per outermost occurrence, so nested renders of one snippet
    // aren't counted twice.
    let mut open: HashMap<String, u32> = HashMap::new();
    let mut stack: Vec<(u32, bool, Option<String>)> = tree.roots.iter().rev().map(|&r| (r, false, None)).collect();
    while let Some((n, exiting, group)) = stack.pop() {
        if exiting {
            if let Some(g) = group {
                *open.get_mut(&g).unwrap() -= 1;
            }
            continue;
        }
        let node = &tree.nodes[n as usize];
        let f = &tree.frames[node.frame as usize];
        let key = &keys[n as usize];
        if key.1 > 0 {
            let e = lines.entry(key.clone()).or_insert_with(|| (Acc::default(), String::new()));
            e.0.self_us += node.self_us;
            if f.line > 0 {
                e.0.calls += 1;
                e.0.total_us += node.total_us;
                if e.1.is_empty() {
                    e.1 = f.name.clone();
                }
            }
        }
        let mut group = None;
        if f.name == "liquid_template" {
            let file = f.file.to_string();
            let depth = open.entry(file.clone()).or_insert(0);
            let a = templates.entry(file.clone()).or_default();
            a.calls += 1;
            a.self_us += node.self_us;
            if *depth == 0 {
                a.total_us += node.total_us;
            }
            *depth += 1;
            group = Some(file);
        }
        stack.push((n, true, group));
        for &c in node.children.iter().rev() {
            stack.push((c, false, None));
        }
    }
    let mut sections = Vec::new();
    let mut blocks = Vec::new();
    let mut snippets = Vec::new();
    let mut apps: HashMap<String, Acc> = HashMap::new();
    for (file, a) in templates {
        let row = |key: String| Row { key, location: String::new(), calls: a.calls, self_us: a.self_us, total_us: a.total_us };
        if let Some(name) = file.strip_prefix("sections/") {
            sections.push(row(name.to_string()));
        } else if let Some(name) = file.strip_prefix("blocks/") {
            blocks.push(row(name.to_string()));
        } else if let Some(name) = file.strip_prefix("snippets/") {
            snippets.push(row(name.to_string()));
        } else if let Some(name) = app_name(&file) {
            let e = apps.entry(name).or_default();
            e.calls += a.calls;
            e.total_us += a.total_us;
        }
    }
    let mut apps: Vec<Row> =
        apps.into_iter().map(|(k, a)| Row { key: k, calls: a.calls, total_us: a.total_us, ..Default::default() }).collect();
    for v in [&mut sections, &mut blocks, &mut snippets, &mut apps] {
        v.sort_by(|a, b| b.total_us.total_cmp(&a.total_us).then(a.key.cmp(&b.key)));
    }
    let mut line_rows: Vec<Row> = lines
        .into_iter()
        .map(|((file, line), (a, name))| {
            let code = source.and_then(|s| s(&file, line)).map(|c| one_line(&c, 200));
            Row {
                key: match code {
                    Some(c) if !c.is_empty() => c,
                    _ => name,
                },
                location: format!("{file}:{line}"),
                calls: a.calls,
                self_us: a.self_us,
                total_us: a.total_us,
            }
        })
        .collect();
    line_rows.sort_by(|a, b| b.self_us.total_cmp(&a.self_us).then(a.location.cmp(&b.location)));
    Summary { total_us: tree.total_us(), nodes: tree.nodes.len(), sections, blocks, snippets, apps, lines: line_rows }
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
    let table = |title: &str, rows: &[Row], o: &mut String| {
        if rows.is_empty() {
            return;
        }
        let _ = writeln!(o, "{:<40}{:>8} {:>6} {:>9}", title, "ms", "share", "renders");
        for r in rows.iter().take(opts.top) {
            let _ = writeln!(o, "  {:<38}{:>8} {:>6} {:>9}", one_line(&r.key, 38), ms(r.total_us), pct(r.total_us, t), r.calls);
        }
        o.push('\n');
    };
    table("Sections", &s.sections, &mut o);
    table("Blocks", &s.blocks, &mut o);
    table("Snippets", &s.snippets, &mut o);
    table("App blocks and embeds", &s.apps, &mut o);
    let _ = writeln!(o, "{:<10}{:>6}  Slowest lines (own time, filters included)", "  ms", "calls");
    for r in s.lines.iter().take(opts.top) {
        let _ = writeln!(o, "  {:>8} {:>6}  {}  {}", ms(r.self_us), r.calls, r.location, one_line(&r.key, 70));
    }
    o
}

pub fn json(s: &Summary) -> serde_json::Value {
    let rows = |v: &[Row], n: usize| -> Vec<serde_json::Value> {
        v.iter()
            .take(n)
            .map(|r| {
                let mut j = serde_json::json!({
                    "name": r.key, "calls": r.calls,
                    "self_ms": (r.self_us / 10.0).round() / 100.0, "total_ms": (r.total_us / 10.0).round() / 100.0,
                });
                if !r.location.is_empty() {
                    j["location"] = serde_json::json!(r.location);
                }
                j
            })
            .collect()
    };
    serde_json::json!({
        "total_ms": (s.total_us / 10.0).round() / 100.0,
        "nodes": s.nodes,
        "sections": rows(&s.sections, usize::MAX),
        "blocks": rows(&s.blocks, usize::MAX),
        "snippets": rows(&s.snippets, usize::MAX),
        "apps": rows(&s.apps, usize::MAX),
        "lines": rows(&s.lines, 1000),
    })
}

/// The change from `base` to `head` by section, block, snippet and line, largest first.
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
        let label = |r: &Row| if own { format!("{}  {}", r.location, r.key) } else { r.key.clone() };
        let mut m: HashMap<String, (f64, f64, u32, u32)> = HashMap::new();
        let mut shown: HashMap<String, String> = HashMap::new();
        for r in a {
            let e = m.entry(key(r)).or_default();
            e.0 += val(r);
            e.2 += r.calls;
            shown.entry(key(r)).or_insert_with(|| label(r));
        }
        for r in b {
            let e = m.entry(key(r)).or_default();
            e.1 += val(r);
            e.3 += r.calls;
            shown.insert(key(r), label(r));
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
    section("Blocks", &base.blocks, &head.blocks, false, &mut o);
    section("Snippets", &base.snippets, &head.snippets, false, &mut o);
    section("Lines (own time)", &base.lines, &head.lines, true, &mut o);
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names() {
        assert_eq!(app_name("shopify://apps/attentive/blocks/attn-tag/8df6").as_deref(), Some("attentive/attn-tag"));
        assert_eq!(app_name("snippets/x"), None);
    }
}
