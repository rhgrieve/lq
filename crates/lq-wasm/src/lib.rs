//! lq's formatter and theme check, for the browser.
//!
//! Every function takes and returns plain strings; results are JSON. The checker works on one
//! file at a time, so checks that need the rest of a theme to say anything useful are left out:
//! see [`CROSS_FILE_CHECKS`].

use lq_check::fixes::fix_all;
use lq_check::fs::MockFileSystem;
use lq_check::registry::{CONFIG_REGISTRY, REGISTRY};
use lq_check::deps::NodeDependencies;
use lq_fmt::Options;
use lq_theme::config::load_builtin_config;
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;

/// Where the playground's theme lives. Only used to build URIs.
const ROOT: &str = "/theme";

/// Checks that report on what's missing from, or unused in, the rest of the theme. With one
/// file they'd fire on nearly every reference.
pub const CROSS_FILE_CHECKS: &[&str] = &[
    "MissingAsset",
    "MissingBlock",
    "MissingContentForArguments",
    "MissingRenderSnippetArguments",
    "MissingSnippet",
    "MissingTemplate",
    "OrphanedSnippet",
    "TranslationKeyExists",
    "UnusedDocParam",
    "UnusedSnippet",
    "ValidRenderSnippetArgumentTypes",
    "UnrecognizedRenderSnippetArguments",
    "ValidStaticBlockType",
];

/// The lq version this was built from.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Formats Liquid like `lq fmt`. `options` is JSON with any of `printWidth`, `tabWidth`,
/// `useTabs`, `singleQuote`. Returns `{"ok":true,"output":...}` or
/// `{"ok":false,"name":...,"message":...,"line":n|null,"column":n|null}`.
#[wasm_bindgen]
pub fn format(source: &str, options: &str) -> String {
    let opts = parse_options(options);
    let result = format_on_big_stack(source.to_string(), opts);
    match result {
        Ok(output) => json!({ "ok": true, "output": output }),
        Err(e) => json!({
            "ok": false,
            "name": e.name,
            "message": e.message,
            "line": e.loc.map(|l| l.0),
            "column": e.loc.map(|l| l.1),
        }),
    }
    .to_string()
}

/// Checks one file like `lq check` with the recommended config. `path` is the file's place in a
/// theme, like `snippets/card.liquid` or `sections/header.liquid`, which decides which checks
/// apply. Returns `{"offenses":[...],"fixed":string|null}`; `fixed` is the source with every safe
/// fix applied, when any offense has one.
#[wasm_bindgen]
pub fn check(source: &str, path: &str) -> String {
    match run_check(source, path) {
        Ok(v) => v.to_string(),
        Err(message) => json!({ "error": message }).to_string(),
    }
}

/// Every check the playground runs, with its docs link, as JSON.
#[wasm_bindgen]
pub fn checks() -> String {
    let config = match load_builtin_config("theme-check:recommended", ROOT, CONFIG_REGISTRY) {
        Ok(c) => c,
        Err(e) => return json!({ "error": e.to_string() }).to_string(),
    };
    let list: Vec<Value> = REGISTRY
        .iter()
        .filter(|d| config.checks.contains(&d.meta.code) && !CROSS_FILE_CHECKS.contains(&d.meta.code))
        .map(|d| json!({ "code": d.meta.code, "description": d.meta.description, "url": d.meta.url }))
        .collect();
    Value::Array(list).to_string()
}

fn parse_options(options: &str) -> Options {
    let mut opts = Options::default();
    let Ok(Value::Object(o)) = serde_json::from_str::<Value>(options) else { return opts };
    if let Some(n) = o.get("printWidth").and_then(Value::as_u64) {
        opts.print_width = n.clamp(20, 400) as usize;
    }
    if let Some(n) = o.get("tabWidth").and_then(Value::as_u64) {
        opts.tab_width = n.clamp(1, 16) as usize;
    }
    if let Some(b) = o.get("useTabs").and_then(Value::as_bool) {
        opts.use_tabs = b;
    }
    if let Some(b) = o.get("singleQuote").and_then(Value::as_bool) {
        opts.single_quote = b;
    }
    opts
}

#[cfg(target_family = "wasm")]
fn format_on_big_stack(source: String, opts: Options) -> Result<String, lq_fmt::FormatError> {
    lq_fmt::format_source(&source, &opts)
}

#[cfg(not(target_family = "wasm"))]
fn format_on_big_stack(source: String, opts: Options) -> Result<String, lq_fmt::FormatError> {
    lq_fmt::with_big_stack(move || lq_fmt::format_source(&source, &opts))
}

fn normalize_path(path: &str) -> String {
    let p = path.trim().trim_start_matches('/');
    if p.is_empty() || p.contains("..") {
        "snippets/playground.liquid".to_string()
    } else if p.ends_with(".liquid") || p.ends_with(".json") {
        p.to_string()
    } else {
        format!("{p}.liquid")
    }
}

fn run_check(source: &str, path: &str) -> Result<Value, String> {
    let path = normalize_path(path);
    let config =
        load_builtin_config("theme-check:recommended", ROOT, CONFIG_REGISTRY).map_err(|e| e.to_string())?;
    let file_uri = lq_json::uri::file_uri(&format!("{ROOT}/{path}"));

    let sources = vec![(file_uri, source.to_string())];
    let theme = lq_check::parse_theme(&sources);

    let mut files = lq_json::Object::new();
    files.insert(path.clone(), lq_json::Value::String(source.to_string()));
    let fs = MockFileSystem::new(files, &config.root_uri);
    let deps = NodeDependencies::with_fs(&theme, &config, ROOT, Box::new(fs));

    let output = lq_check::check(&theme, &config, &deps);
    let mut offenses: Vec<_> =
        output.offenses.into_iter().filter(|o| !CROSS_FILE_CHECKS.contains(&o.check)).collect();
    offenses.sort_by_key(|o| (o.start_byte, o.end_byte));

    let fixes: Vec<_> = offenses.iter().filter_map(|o| o.fix.as_ref()).collect();
    let fixed = if fixes.is_empty() { None } else { fix_all(source, &fixes).ok() };

    let list: Vec<Value> = offenses
        .iter()
        .map(|o| {
            let url = REGISTRY.iter().find(|d| d.meta.code == o.check).and_then(|d| d.meta.url);
            // Each suggestion as the whole source with it applied: the page swaps it in.
            let suggestions: Vec<Value> = o
                .suggest
                .iter()
                .flatten()
                .filter_map(|s| {
                    let fixed = fix_all(source, &[&s.fix]).ok()?;
                    Some(json!({ "message": s.message, "fixed": fixed }))
                })
                .collect();
            json!({
                "suggestions": suggestions,
                "check": o.check,
                "severity": o.severity.as_str(),
                "message": o.message,
                "start": { "line": o.start.line, "character": o.start.character, "index": o.start.index },
                "end": { "line": o.end.line, "character": o.end.character, "index": o.end.index },
                "fixable": o.fix.is_some(),
                "url": url,
            })
        })
        .collect();
    Ok(json!({ "path": path, "offenses": list, "fixed": fixed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: String) -> Value {
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn formats() {
        let out = parse(format("<div><p>{{x}}</p></div>", "{}"));
        assert_eq!(out["ok"], true);
        assert_eq!(out["output"], "<div>\n  <p>{{ x }}</p>\n</div>\n");
    }

    #[test]
    fn format_options_apply() {
        let src = "<div>\n<p>{{ x }}</p>\n</div>";
        let out = parse(format(src, r#"{"useTabs":true}"#));
        assert!(out["output"].as_str().unwrap().contains("\n\t<p>"), "{out}");
    }

    #[test]
    fn format_reports_parse_errors() {
        let out = parse(format("{% toString %}", "{}"));
        assert_eq!(out["ok"], false);
        assert!(out["message"].as_str().unwrap().len() > 0);
    }

    #[test]
    fn checks_one_file() {
        let out = parse(check("{{ product.featured_image | img_url: '100x' }}\n", "snippets/a.liquid"));
        let codes: Vec<&str> = out["offenses"].as_array().unwrap().iter().map(|o| o["check"].as_str().unwrap()).collect();
        assert!(codes.contains(&"DeprecatedFilter"), "{out}");
    }

    #[test]
    fn leaves_out_cross_file_checks() {
        let out = parse(check("{% render 'missing' %}{{ 'x' | t }}\n", "snippets/a.liquid"));
        for o in out["offenses"].as_array().unwrap() {
            assert!(!CROSS_FILE_CHECKS.contains(&o["check"].as_str().unwrap()), "{out}");
        }
    }

    #[test]
    fn applies_fixes() {
        let out = parse(check("{{ x | img_url: '100x' }}\n", "snippets/a.liquid"));
        if out["offenses"].as_array().unwrap().iter().any(|o| o["fixable"] == true) {
            assert!(out["fixed"].is_string(), "{out}");
        }
    }

    #[test]
    fn suggestions_carry_the_fixed_source() {
        let src = "<script src=\"{{ 'a.js' | asset_url }}\"></script>\n";
        let out = parse(check(src, "snippets/a.liquid"));
        let o = out["offenses"].as_array().unwrap().iter().find(|o| o["check"] == "ParserBlockingScript").unwrap();
        let fixed = o["suggestions"][0]["fixed"].as_str().unwrap();
        assert!(fixed.contains("defer") || fixed.contains("async"), "{o}");
    }

    #[test]
    fn lists_checks() {
        let list: Value = serde_json::from_str(&checks()).unwrap();
        assert!(list.as_array().unwrap().len() > 20);
    }
}
