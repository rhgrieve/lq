//! Runs upstream's language-neutral check specs (`specs/checks/*.yml` in Shopify/theme-tools)
//! the way their `specs.spec.ts` does: a mock theme at `file:/`, the case's docset, references
//! and settings, and every offense compared by file, message, start, highlight, fix and
//! suggestions.

use crate::api::Severity;
use crate::fixes::{apply_fix_to_string, fix_all, fix_edits, Fix};
use crate::golden::MockDependencies;
use crate::registry::REGISTRY;
use crate::run::{check_with_registry, Offense};
use crate::source::{to_source_code, Theme};
use lq_json::{uri, Object, Value};
use lq_theme::config::{Config, Mode};
use lq_theme::divergence::Divergences;

const ROOT_URI: &str = "file:///";

/// What became of one case.
#[derive(Debug)]
pub enum CaseOutcome {
    Passed,
    Failed(String),
    /// The case needs something lq doesn't have, such as a remote file's size.
    Unsupported(String),
}

/// A spec's `Text`: a string, or parts to join (`{ repeat, times }`).
fn expand(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|p| match p {
                Value::String(s) => s.clone(),
                _ => {
                    let times = p.get("times").and_then(Value::as_f64).unwrap_or(0.0) as usize;
                    p.get("repeat").and_then(Value::as_str).unwrap_or("").repeat(times)
                }
            })
            .collect(),
        _ => String::new(),
    }
}

/// `text.slice(start, end)` in UTF-16 units.
fn slice_utf16(text: &str, start: u32, end: u32) -> String {
    let (mut i, mut out) = (0u32, String::new());
    for c in text.chars() {
        if i >= end {
            break;
        }
        if i >= start {
            out.push(c);
        }
        i += c.len_utf16() as u32;
    }
    out
}

fn severity_name(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    }
}

fn apply(source: &str, fix: &Fix) -> String {
    match fix_edits(source, fix).and_then(|e| apply_fix_to_string(source, &e)) {
        Ok(s) => s,
        Err(e) => format!("<fix threw: {e}>"),
    }
}

/// An offense as a spec states it, as one comparable string.
fn spec_offense(o: &Offense, sources: &[(String, String)], with_check: bool) -> String {
    let rel = uri::relative(&o.uri, ROOT_URI);
    let text = sources.iter().find(|(u, _)| *u == o.uri).map_or("", |(_, s)| s.as_str());
    let mut out = Object::new();
    if with_check {
        out.insert("check", Value::String(o.check.to_string()));
    }
    out.insert("file", Value::String(rel));
    out.insert("message", Value::String(o.message.clone()));
    out.insert(
        "start",
        Value::Array(vec![Value::Number(f64::from(o.start.line)), Value::Number(f64::from(o.start.character))]),
    );
    out.insert("highlight", Value::String(slice_utf16(text, o.start.index, o.end.index)));
    if let Some(fix) = &o.fix {
        out.insert("fix", Value::String(apply(text, fix)));
    }
    if let Some(suggest) = &o.suggest {
        out.insert(
            "suggest",
            Value::Array(
                suggest
                    .iter()
                    .map(|s| {
                        let mut so = Object::new();
                        so.insert("message", Value::String(s.message.clone()));
                        so.insert("result", Value::String(apply(text, &s.fix)));
                        Value::Object(so)
                    })
                    .collect(),
            ),
        );
    }
    Value::Object(out).to_json()
}

/// The expected offense, in the same form as [`spec_offense`].
fn expected_offense(o: &Value) -> String {
    let mut out = Object::new();
    if let Some(c) = o.get("check") {
        out.insert("check", c.clone());
    }
    for k in ["file", "message", "start"] {
        out.insert(k, o.get(k).cloned().unwrap_or(Value::Null));
    }
    out.insert("highlight", Value::String(expand(o.get("highlight").unwrap_or(&Value::Null))));
    if let Some(f) = o.get("fix") {
        out.insert("fix", Value::String(expand(f)));
    }
    if let Some(s) = o.get("suggest").and_then(Value::as_array) {
        out.insert(
            "suggest",
            Value::Array(
                s.iter()
                    .map(|x| {
                        let mut so = Object::new();
                        so.insert("message", x.get("message").cloned().unwrap_or(Value::Null));
                        so.insert("result", Value::String(expand(x.get("result").unwrap_or(&Value::Null))));
                        Value::Object(so)
                    })
                    .collect(),
            ),
        );
    }
    Value::Object(out).to_json()
}

/// The docset a case names (`docsets/<name>.yml`, already parsed) or spells out.
fn docset(case: &Value, docsets: &dyn Fn(&str) -> Option<Value>) -> Value {
    let data = match case.get("docset") {
        None => return Value::Null,
        Some(Value::String(name)) => docsets(name).unwrap_or(Value::Null),
        Some(v) => v.clone(),
    };
    let mut out = Object::new();
    for k in ["filters", "objects", "tags"] {
        out.insert(k, data.get(k).cloned().unwrap_or(Value::Array(Vec::new())));
    }
    out.insert("liquidDrops", data.get("objects").cloned().unwrap_or(Value::Array(Vec::new())));
    out.insert("systemTranslations", data.get("systemTranslations").cloned().unwrap_or(Value::Object(Object::new())));
    Value::Object(out)
}

/// Runs one case of a spec whose top-level `check` is `spec_check`.
pub fn run_case(
    spec_check: Option<&str>,
    spec_severity: Option<&str>,
    case: &Value,
    docsets: &dyn Fn(&str) -> Option<Value>,
    divergences: Divergences,
) -> CaseOutcome {
    // A file at a URL is a remote file: the check only ever sees its size.
    let is_remote = |k: &str| k.starts_with("http://") || k.starts_with("https://");
    let all_files = case.get("files").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut files = Object::new();
    let mut remote_sizes = Object::new();
    for (k, v) in all_files.iter() {
        if is_remote(k) {
            remote_sizes.insert(k, Value::Number(expand(v).len() as f64));
        } else {
            files.insert(k, v.clone());
        }
    }
    let listed: Vec<String> = match case.get("checks").and_then(Value::as_array) {
        Some(l) => l.iter().filter_map(Value::as_str).map(String::from).collect(),
        None => spec_check.into_iter().map(String::from).collect(),
    };
    let codes: Vec<&'static str> =
        REGISTRY.iter().filter(|d| listed.iter().any(|c| c == d.meta.code)).map(|d| d.meta.code).collect();
    if let Some(missing) = listed.iter().find(|c| !codes.contains(&c.as_str())) {
        return CaseOutcome::Unsupported(format!("no check {missing}"));
    }
    if let Some(d) = REGISTRY.iter().find(|d| codes.contains(&d.meta.code) && !d.imp.is_implemented()) {
        return CaseOutcome::Unsupported(format!("{} not implemented", d.meta.code));
    }

    let sources: Vec<(String, String)> =
        files.iter().map(|(k, v)| (uri::join(ROOT_URI, &[k]), expand(v))).collect();
    let theme: Theme<'_> = sources.iter().map(|(u, s)| to_source_code(u, s)).collect();
    let mut theme_desc = Object::new();
    for (k, v) in files.iter() {
        theme_desc.insert(k, Value::String(expand(v)));
    }

    // `getReferences`/`getDependencies` from `references`, keyed by target and source.
    let (mut refs, mut deps_of) = (Object::new(), Object::new());
    for r in case.get("references").and_then(Value::as_array).unwrap_or(&[]) {
        let loc = |k: &str| {
            let mut o = Object::new();
            o.insert("uri", Value::String(uri::join(ROOT_URI, &[r.get(k).and_then(Value::as_str).unwrap_or("")])));
            Value::Object(o)
        };
        let mut o = Object::new();
        o.insert("source", loc("source"));
        o.insert("target", loc("target"));
        o.insert("type", r.get("type").cloned().unwrap_or(Value::String("direct".into())));
        let reference = Value::Object(o);
        for (table, end) in [(&mut refs, "target"), (&mut deps_of, "source")] {
            let key = reference.get(end).and_then(|l| l.get("uri")).and_then(Value::as_str).unwrap().to_string();
            match table.get_mut(&key) {
                Some(Value::Array(a)) => a.push(reference.clone()),
                _ => {
                    table.insert(&key, Value::Array(vec![reference.clone()]));
                }
            }
        }
    }
    let mut recorded = Object::new();
    recorded.insert("themeDocset", docset(case, docsets));
    recorded.insert("getReferences", Value::Object(refs));
    recorded.insert("getDependencies", Value::Object(deps_of));
    recorded.insert("remoteFileSize", Value::Object(remote_sizes));
    let recorded = Value::Object(recorded);

    let mode = match case.get("mode").and_then(Value::as_str) {
        Some("app") => Mode::App,
        _ => Mode::Theme,
    };
    let settings = case.get("config").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut config = Config::new_for_test(mode, settings, codes.clone(), ROOT_URI.to_string(), None);
    config.divergences = divergences;
    let deps = MockDependencies::new(&theme, mode, &theme_desc, &recorded);
    let output = check_with_registry(&theme, &config, &deps, REGISTRY);
    if let Some((code, u, e)) = output.errors.first() {
        return CaseOutcome::Failed(format!("{code} threw on {u}: {}: {}", e.name, e.message));
    }

    let with_check = case.get("checks").is_some();
    let mut got: Vec<String> = output.offenses.iter().map(|o| spec_offense(o, &sources, with_check)).collect();
    let mut want: Vec<String> =
        case.get("offenses").and_then(Value::as_array).unwrap_or(&[]).iter().map(expected_offense).collect();
    got.sort();
    want.sort();
    if got != want {
        let missing: Vec<&String> = want.iter().filter(|w| !got.contains(w)).collect();
        let extra: Vec<&String> = got.iter().filter(|g| !want.contains(g)).collect();
        return CaseOutcome::Failed(format!("missing {missing:?}\n      extra {extra:?}"));
    }

    // The spec's severity, which every offense of its check must have.
    if let Some(sev) = spec_check.and(spec_severity) {
        if let Some(o) = output.offenses.iter().find(|o| o.check == spec_check.unwrap() && severity_name(o.severity) != sev) {
            return CaseOutcome::Failed(format!("severity {} for {}", severity_name(o.severity), o.check));
        }
    }

    // `autofix`: each file with several fixes, all applied at once.
    let mut fixed = Object::new();
    for (u, text) in &sources {
        let fixes: Vec<&Fix> = output.offenses.iter().filter(|o| o.uri == *u).filter_map(|o| o.fix.as_ref()).collect();
        if fixes.len() > 1 {
            let result = fix_all(text, &fixes).unwrap_or_else(|e| format!("<autofix threw: {e}>"));
            fixed.insert(&uri::relative(u, ROOT_URI), Value::String(result));
        }
    }
    let sorted = |o: Option<&Object>| {
        let mut v: Vec<(String, String)> =
            o.map(|o| o.iter().map(|(k, v)| (k.to_string(), expand(v))).collect()).unwrap_or_default();
        v.sort();
        v
    };
    let want_fixed = sorted(case.get("autofix").and_then(Value::as_object));
    let got_fixed = sorted(Some(&fixed));
    if want_fixed != got_fixed {
        return CaseOutcome::Failed(format!("autofix: want {want_fixed:?} got {got_fixed:?}"));
    }
    CaseOutcome::Passed
}
