//! Debug entry for `scripts/parity/jsonschema.mjs`.
//!
//! `lq-json-debug validate --docs <dir> --mode theme|app [--manifest <file>] --cases <file>`
//! `lq-json-debug bench --docs <dir> --mode theme|app --cases <file>`
//! `lq-json-debug url --cases <file>` (JSON `[input, base]` per line)

use lq_json::js::quote;
use lq_json::validator::{JsonValidator, SchemaDefinition, Severity};
use lq_json::{jsonc, Value};
use std::io::Write;
use std::path::Path;
use std::time::Instant;

const SCHEMA_ROOT: &str =
    "https://raw.githubusercontent.com/Shopify/theme-liquid-docs/main/schemas";

fn definitions(docs: &str, mode: &str, manifest: Option<&str>) -> Vec<SchemaDefinition> {
    let file = manifest.map(str::to_string).unwrap_or_else(|| {
        let name = if mode == "app" {
            "manifest_theme_app_extension.json"
        } else {
            "manifest_theme.json"
        };
        format!("{docs}/{name}")
    });
    let text = std::fs::read_to_string(&file).expect("manifest");
    let manifest = jsonc::parse_json(&text).expect("manifest json");
    let schemas = manifest
        .get("schemas")
        .and_then(Value::as_array)
        .expect("manifest schemas");
    schemas
        .iter()
        .map(|entry| {
            let uri = entry.get("uri").and_then(Value::as_str).expect("uri");
            let basename = uri.rsplit('/').next().unwrap_or(uri);
            let file_match = entry
                .get("fileMatch")
                .and_then(Value::as_array)
                .map(|patterns| {
                    patterns
                        .iter()
                        .filter_map(|p| p.as_str().map(str::to_string))
                        .collect()
                });
            SchemaDefinition {
                uri: format!("{SCHEMA_ROOT}/{uri}"),
                file_match,
                schema: std::fs::read_to_string(Path::new(docs).join(basename)).expect("schema"),
            }
        })
        .collect()
}

fn utf16_offset(text: &str, byte: u32) -> usize {
    text[..byte as usize].encode_utf16().count()
}

fn error_json(name: &str, message: &str) -> String {
    format!(
        "{{\"name\":{},\"message\":{}}}",
        quote(name),
        quote(message)
    )
}

fn cases(path: &str) -> Vec<Value> {
    let text = std::fs::read_to_string(path).expect("cases");
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|l| jsonc::parse_json(l).expect("case json"))
        .collect()
}

fn validate_line(validator: &JsonValidator, uri: &str, json: &str) -> String {
    let mut out = String::from("{");
    match validator.validate(uri, json) {
        Ok(diagnostics) => {
            out.push_str("\"diagnostics\":[");
            for (i, d) in diagnostics.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&format!(
                    "{{\"message\":{},\"start\":{},\"end\":{},\"severity\":{}}}",
                    quote(&d.message),
                    utf16_offset(json, d.start),
                    utf16_offset(json, d.end),
                    if d.severity == Severity::Error { 1 } else { 2 },
                ));
            }
            out.push(']');
        }
        Err(e) => out.push_str(&format!("\"error\":{}", error_json(&e.name, &e.message))),
    }
    match validator.is_valid(uri, json) {
        Ok(valid) => out.push_str(&format!(",\"isValid\":{valid}")),
        Err(e) => out.push_str(&format!(
            ",\"isValidError\":{}",
            error_json(&e.name, &e.message)
        )),
    }
    out.push('}');
    out
}

fn arg<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("");
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    if command == "url" {
        for case in cases(arg(&args, "--cases").expect("--cases")) {
            let pair = case.as_array().expect("pair");
            let (input, base) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
            let result = lq_json::validator::resolve_url_for_debug(input, base);
            let line = match result {
                Some(url) => quote(&url),
                None => "null".to_string(),
            };
            writeln!(out, "{line}").unwrap();
        }
        return;
    }
    let docs = arg(&args, "--docs").expect("--docs");
    let mode = arg(&args, "--mode").unwrap_or("theme");
    let manifest = arg(&args, "--manifest");
    let cases = cases(arg(&args, "--cases").expect("--cases"));
    let validator = JsonValidator::new(definitions(docs, mode, manifest));
    let field = |c: &Value, k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match command {
        "validate" => {
            for case in &cases {
                let line = validate_line(&validator, &field(case, "uri"), &field(case, "json"));
                writeln!(out, "{line}").unwrap();
            }
        }
        "bench" => {
            for case in &cases {
                let (uri, json) = (field(case, "uri"), field(case, "json"));
                let fresh = JsonValidator::new(definitions(docs, mode, manifest));
                let t = Instant::now();
                let first = fresh.validate(&uri, &json);
                let first_time = t.elapsed();
                let t = Instant::now();
                let rounds = 2000;
                for _ in 0..rounds {
                    std::hint::black_box(validator.validate(&uri, &json)).ok();
                }
                let later = t.elapsed() / rounds;
                writeln!(
                    out,
                    "{} bytes={} diagnostics={:?} first={:?} subsequent={:?}",
                    uri.rsplit('/').next().unwrap_or(""),
                    json.len(),
                    first.map(|d| d.len()),
                    first_time,
                    later
                )
                .unwrap();
            }
        }
        _ => {
            eprintln!("usage: lq-json-debug validate|bench|url ...");
            std::process::exit(2);
        }
    }
}
