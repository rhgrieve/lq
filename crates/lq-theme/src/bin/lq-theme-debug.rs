//! Differential-testing front end for `lq-theme`. See `scripts/parity`.

use lq_json::js::quote;
use lq_json::value::{Object, Value};
use lq_theme::config::{load_config, CheckInfo, Config, DefaultValue, SchemaProp, SchemaType};
use lq_theme::discovery::{get_theme_files, sort_paths};
use lq_theme::ignore::is_ignored;
use lq_theme::minimatch::Minimatch;
use lq_theme::yaml;
use std::io::Write;

fn leak<T>(v: T) -> &'static T {
    Box::leak(Box::new(v))
}

fn leak_str(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn schema_from(o: &Object) -> &'static [(&'static str, SchemaProp)] {
    let v: Vec<(&'static str, SchemaProp)> = o.iter().map(|(k, p)| (leak_str(k), prop_from(p))).collect();
    Box::leak(v.into_boxed_slice())
}

fn prop_from(v: &Value) -> SchemaProp {
    let ty = match v.get("type").and_then(Value::as_str) {
        Some("string") => SchemaType::String,
        Some("number") => SchemaType::Number,
        Some("boolean") => SchemaType::Boolean,
        Some("object") => SchemaType::Object,
        _ => SchemaType::Array,
    };
    let default = v.get("default").map(|d| match d {
        Value::Null => DefaultValue::Null,
        Value::Bool(b) => DefaultValue::Bool(*b),
        Value::Number(n) => DefaultValue::Num(*n),
        Value::String(s) => DefaultValue::Str(leak_str(s)),
        Value::Array(a) if a.is_empty() => DefaultValue::EmptyArray,
        Value::Array(a) => {
            let items: Vec<&'static str> = a.iter().filter_map(Value::as_str).map(leak_str).collect();
            DefaultValue::StrArray(Box::leak(items.into_boxed_slice()))
        }
        Value::Object(_) => DefaultValue::Null,
    });
    SchemaProp {
        ty,
        default,
        optional: v.get("optional").and_then(Value::as_bool).unwrap_or(false),
        properties: v.get("properties").and_then(Value::as_object).map_or(&[], schema_from),
        item_type: v.get("itemType").map(|i| leak(prop_from(i))),
    }
}

fn registry(path: &str) -> &'static [CheckInfo] {
    let text = std::fs::read_to_string(path).expect("registry file");
    let v = lq_json::parse_json(&text).expect("registry json");
    let checks: Vec<CheckInfo> = v
        .as_array()
        .expect("registry array")
        .iter()
        .map(|c| CheckInfo {
            code: leak_str(c.get("code").and_then(Value::as_str).expect("code")),
            aliases: {
                let a: Vec<&'static str> = c
                    .get("aliases")
                    .and_then(Value::as_array)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Value::as_str)
                    .map(leak_str)
                    .collect();
                Box::leak(a.into_boxed_slice())
            },
            schema: c.get("schema").and_then(Value::as_object).map_or(&[], schema_from),
        })
        .collect();
    Box::leak(checks.into_boxed_slice())
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn error_json(name: &str, message: &str) -> String {
    format!("{{\"error\":{{\"name\":{},\"message\":{}}}}}", quote(name), quote(message))
}

fn config_json(c: &Config) -> String {
    let mut o = Object::new();
    o.insert("context", Value::String(c.context.as_str().into()));
    o.insert("settings", Value::Object(c.settings.clone()));
    o.insert("checks", Value::Array(c.checks.iter().map(|s| Value::String((*s).into())).collect()));
    o.insert("rootUri", Value::String(c.root_uri.clone()));
    if let Some(i) = c.ignore_values() {
        o.insert("ignore", Value::Array(i.to_vec()));
    }
    Value::Object(o).to_json()
}

fn load(args: &[String]) -> Result<Config, lq_theme::config::ConfigError> {
    let root = arg(args, "--root").expect("--root");
    let reg = registry(&arg(args, "--registry").expect("--registry"));
    load_config(arg(args, "--config").as_deref(), &root, reg)
}

fn cases(args: &[String]) -> Vec<Value> {
    let text = std::fs::read_to_string(arg(args, "--cases").expect("--cases")).expect("cases file");
    text.lines().filter(|l| !l.is_empty()).map(|l| lq_json::parse_json(l).expect("case json")).collect()
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    match args.first().map(String::as_str) {
        Some("config") => {
            let line = match load(&args) {
                Ok(c) => config_json(&c),
                Err(e) => error_json(&e.name, &e.message),
            };
            writeln!(out, "{line}").ok();
        }
        Some("files") => match load(&args) {
            Ok(c) => {
                for f in get_theme_files(&c) {
                    writeln!(out, "{}", quote(&f)).ok();
                }
            }
            Err(e) => {
                writeln!(out, "{}", error_json(&e.name, &e.message)).ok();
            }
        },
        // Each case: {path, pattern}. Prints 1, 0, or `!` when upstream would throw.
        Some("minimatch") => {
            for c in cases(&args) {
                let mm = Minimatch::new(s(&c, "pattern"));
                let r = if mm.error().is_some() {
                    "!"
                } else if mm.matches(s(&c, "path")) {
                    "1"
                } else {
                    "0"
                };
                writeln!(out, "{r}").ok();
            }
        }
        // Each case: {uri, rootUri, ignore, checkIgnore?}. Prints 1 or 0.
        Some("ignored") => {
            for c in cases(&args) {
                let strs = |k: &str| -> Vec<String> {
                    c.get(k)
                        .and_then(Value::as_array)
                        .unwrap_or(&[])
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                };
                let mut settings = Object::new();
                let code = c.get("checkIgnore").map(|ci| {
                    let mut set = Object::new();
                    set.insert("ignore", ci.clone());
                    settings.insert("Check", Value::Object(set));
                    "Check"
                });
                let cfg = Config::new_for_test(
                    lq_theme::config::Mode::Theme,
                    settings,
                    Vec::new(),
                    s(&c, "rootUri").to_string(),
                    Some(strs("ignore")),
                );
                writeln!(out, "{}", u8::from(is_ignored(s(&c, "uri"), &cfg, code))).ok();
            }
        }
        // Each case: {src} parses and prints the result, or {value} stringifies.
        Some("yaml") => {
            for c in cases(&args) {
                let line = if let Some(v) = c.get("value") {
                    format!("{{\"str\":{}}}", quote(&yaml::stringify(v)))
                } else {
                    match yaml::parse(s(&c, "src")) {
                        Ok(v) => {
                            let mut o = Object::new();
                            o.insert("value", v);
                            Value::Object(o).to_json()
                        }
                        Err(e) => error_json(&e.name, &e.message),
                    }
                };
                writeln!(out, "{line}").ok();
            }
        }
        // Each case: {list: [...]}; prints the stable-sorted list as JSON.
        Some("sort") => {
            for c in cases(&args) {
                let list: Vec<String> = c
                    .get("list")
                    .and_then(Value::as_array)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect();
                let sorted: Vec<Value> = sort_paths(list).into_iter().map(Value::String).collect();
                writeln!(out, "{}", Value::Array(sorted).to_json()).ok();
            }
        }
        _ => {
            eprintln!("usage: lq-theme-debug config|files|minimatch|ignored|yaml|sort ...");
            std::process::exit(2);
        }
    }
    out.flush().ok();
}
