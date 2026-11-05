//! Formats for tools: `short`, `github` and `json-rich`. All on stdout, and never styled.

use super::fold::fold;
use super::{clean_char, sanitize, Report};
use lq_check::fixes::{Fix, JsonPatch};
use lq_check::registry::REGISTRY;
use lq_check::{Offense, Position, Severity};
use lq_json::{uri, Object, Value};

/// One line per offense: `path:line:col: severity Check: message`.
pub fn short(report: &Report<'_>) -> String {
    let mut out = String::new();
    for o in &report.offenses {
        out.push_str(&format!("{}: {} {}: {}\n", report.location(o), o.severity.as_str(), o.check, sanitize(&o.message)));
    }
    out
}

/// Newlines survive, as the commands' escape for them. Other control characters don't.
fn escape_data(text: &str) -> String {
    let clean: String = text.chars().map(|c| if matches!(c, '\n' | '\r') { c } else { clean_char(c) }).collect();
    clean.replace('%', "%25").replace('\r', "%0D").replace('\n', "%0A")
}

fn escape_property(text: &str) -> String {
    escape_data(text).replace(':', "%3A").replace(',', "%2C")
}

/// GitHub Actions workflow commands, which become annotations on a pull request.
pub fn github(report: &Report<'_>) -> String {
    let mut out = String::new();
    for o in &report.offenses {
        let command = match o.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "notice",
        };
        out.push_str(&format!(
            "::{command} file={},line={},col={},endLine={},endColumn={},title={}::{}\n",
            escape_property(&report.display_path(&o.uri)),
            o.start.line + 1,
            o.start.character + 1,
            o.end.line + 1,
            o.end.character + 1,
            escape_property(o.check),
            escape_data(&o.message),
        ));
    }
    out
}

fn number(n: usize) -> Value {
    Value::Number(n as f64)
}

fn position(p: &Position, byte: u32) -> Value {
    let mut o = Object::new();
    o.insert("line", number(p.line as usize + 1));
    o.insert("column", number(p.character as usize + 1));
    o.insert("byte", number(byte as usize));
    Value::Object(o)
}

fn fix_value(fix: &Fix) -> Value {
    let mut o = Object::new();
    match fix {
        Fix::Edits(edits) => {
            o.insert("kind", Value::String("edits".into()));
            let list = edits.iter().map(|e| {
                let mut m = Object::new();
                m.insert("startByte", number(e.start as usize));
                m.insert("endByte", number(e.end as usize));
                m.insert("insert", Value::String(e.insert.clone()));
                Value::Object(m)
            });
            o.insert("edits", Value::Array(list.collect()));
        }
        Fix::Json(patches) => {
            o.insert("kind", Value::String("json".into()));
            let list = patches.iter().map(|p| {
                let mut m = Object::new();
                match p {
                    JsonPatch::Add { path, value } => {
                        m.insert("op", Value::String("add".into()));
                        m.insert("path", Value::String(path.clone()));
                        m.insert("value", value.clone());
                    }
                    JsonPatch::Remove { path } => {
                        m.insert("op", Value::String("remove".into()));
                        m.insert("path", Value::String(path.clone()));
                    }
                }
                Value::Object(m)
            });
            o.insert("patches", Value::Array(list.collect()));
        }
    }
    Value::Object(o)
}

fn diagnostic(report: &Report<'_>, o: &Offense, group: usize) -> Value {
    let mut d = Object::new();
    d.insert("check", Value::String(o.check.to_string()));
    d.insert("severity", Value::String(o.severity.as_str().to_string()));
    d.insert("message", Value::String(o.message.clone()));
    d.insert("file", Value::String(report.display_path(&o.uri)));
    d.insert("absolutePath", Value::String(uri::fs_path(&o.uri)));
    d.insert("start", position(&o.start, o.start_byte));
    d.insert("end", position(&o.end, o.end_byte));
    d.insert("label", o.hints.label.clone().map_or(Value::Null, Value::String));
    let notes = o.hints.notes.iter().map(|n| {
        let mut m = Object::new();
        m.insert("message", Value::String(n.message.clone()));
        if let Some(r) = &n.related {
            m.insert("file", Value::String(report.display_path(&r.uri)));
            m.insert("startByte", number(r.start as usize));
            m.insert("endByte", number(r.end as usize));
        }
        Value::Object(m)
    });
    d.insert("notes", Value::Array(notes.collect()));
    d.insert("fix", o.fix.as_ref().map_or(Value::Null, fix_value));
    let blocked = o.fix.as_ref().and_then(|_| report.unfixable.get(o.uri.as_str()));
    d.insert("fixBlockedBy", blocked.map_or(Value::Null, |reason| Value::String(reason.clone())));
    let suggestions = o.suggest.as_deref().unwrap_or(&[]).iter().map(|s| {
        let mut m = Object::new();
        m.insert("message", Value::String(s.message.clone()));
        m.insert("fix", fix_value(&s.fix));
        Value::Object(m)
    });
    d.insert("suggestions", Value::Array(suggestions.collect()));
    let url = REGISTRY.iter().find(|c| c.meta.code == o.check).and_then(|c| c.meta.url);
    d.insert("docsUrl", url.map_or(Value::Null, |u| Value::String(u.to_string())));
    d.insert("foldGroup", number(group));
    Value::Object(d)
}

/// Everything the text report shows, as data. Unlike `--output json`, which is upstream's
/// format and stays exactly that, this one is `lq`'s own.
pub fn json_rich(report: &Report<'_>) -> String {
    let groups = fold(&report.offenses);
    let mut diagnostics = Vec::with_capacity(report.offenses.len());
    for (i, group) in groups.iter().enumerate() {
        diagnostics.push(diagnostic(report, group.first, i));
        diagnostics.extend(group.rest.iter().map(|o| diagnostic(report, o, i)));
    }
    let count = |s: Severity| number(report.offenses.iter().filter(|o| o.severity == s).count());
    let mut out = Object::new();
    out.insert("schemaVersion", number(1));
    out.insert("valid", Value::Bool(report.valid));
    out.insert("root", Value::String(report.root.to_string()));
    out.insert("filesInspected", number(report.inspected));
    out.insert("errorCount", count(Severity::Error));
    out.insert("warningCount", count(Severity::Warning));
    out.insert("infoCount", count(Severity::Info));
    out.insert("diagnostics", Value::Array(diagnostics));
    Value::Object(out).to_json()
}
