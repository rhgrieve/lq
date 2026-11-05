//! Generates static lookup tables from `data/docs` so the docs cost nothing at
//! startup: no JSON is parsed at runtime for filters, objects, tags or system
//! translations. Only the fields checks read are kept.

use lq_json::value::Value;
use std::fmt::Write as _;
use std::path::Path;

fn lit(s: &str) -> String {
    format!("{s:?}")
}

fn opt_str(v: Option<&Value>) -> String {
    match v.and_then(Value::as_str) {
        Some(s) if !s.is_empty() => format!("Some({})", lit(s)),
        _ => "None".into(),
    }
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(Value::is_truthy)
}

fn parameters(v: Option<&Value>) -> String {
    let mut out = String::from("&[");
    for p in v.and_then(Value::as_array).unwrap_or(&[]) {
        let types: Vec<String> = p
            .get("types")
            .and_then(Value::as_array)
            .unwrap_or(&[])
            .iter()
            .filter_map(Value::as_str)
            .map(lit)
            .collect();
        let _ = write!(
            out,
            "Parameter{{name:{},positional:{},required:{},types:&[{}]}},",
            lit(p.get("name").and_then(Value::as_str).unwrap_or("")),
            truthy(p.get("positional")),
            truthy(p.get("required")),
            types.join(",")
        );
    }
    out.push(']');
    out
}

fn return_types(v: Option<&Value>) -> String {
    let mut out = String::from("&[");
    for r in v.and_then(Value::as_array).unwrap_or(&[]) {
        let s = |k: &str| lit(r.get(k).and_then(Value::as_str).unwrap_or(""));
        let _ = write!(out, "ReturnType{{ty:{},name:{},array_value:{}}},", s("type"), s("name"), s("array_value"));
    }
    out.push(']');
    out
}

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let docs = Path::new(&manifest).join("../../data/docs");
    println!("cargo:rerun-if-changed={}", docs.display());
    println!("cargo:rerun-if-changed=build.rs");
    let load = |name: &str| -> Value {
        let text = std::fs::read_to_string(docs.join(name)).unwrap();
        lq_json::parse_json(&text).unwrap()
    };

    let mut out = String::new();

    out.push_str("pub static FILTERS: &[FilterEntry] = &[\n");
    for f in load("filters.json").as_array().unwrap() {
        let _ = writeln!(
            out,
            "FilterEntry{{name:{},deprecated:{},deprecation_reason:{},parameters:{},return_type:{}}},",
            lit(f.get("name").and_then(Value::as_str).unwrap()),
            truthy(f.get("deprecated")),
            opt_str(f.get("deprecation_reason")),
            parameters(f.get("parameters")),
            return_types(f.get("return_type")),
        );
    }
    out.push_str("];\n");

    out.push_str("pub static OBJECTS: &[ObjectEntry] = &[\n");
    for o in load("objects.json").as_array().unwrap() {
        let access = match o.get("access") {
            Some(a) if a.is_truthy() => {
                let parents: Vec<String> = a
                    .get("parents")
                    .and_then(Value::as_array)
                    .unwrap_or(&[])
                    .iter()
                    .map(|p| {
                        format!(
                            "Parent{{object:{},property:{}}}",
                            lit(p.get("object").and_then(Value::as_str).unwrap_or("")),
                            lit(p.get("property").and_then(Value::as_str).unwrap_or(""))
                        )
                    })
                    .collect();
                let template: Vec<String> = a
                    .get("template")
                    .and_then(Value::as_array)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Value::as_str)
                    .map(lit)
                    .collect();
                format!(
                    "Some(Access{{global:{},parents:&[{}],template:&[{}]}})",
                    truthy(a.get("global")),
                    parents.join(","),
                    template.join(",")
                )
            }
            _ => "None".into(),
        };
        let _ = writeln!(
            out,
            "ObjectEntry{{name:{},deprecated:{},deprecation_reason:{},access:{},return_type:{}}},",
            lit(o.get("name").and_then(Value::as_str).unwrap()),
            truthy(o.get("deprecated")),
            opt_str(o.get("deprecation_reason")),
            access,
            return_types(o.get("return_type")),
        );
    }
    out.push_str("];\n");

    out.push_str("pub static TAGS: &[TagEntry] = &[\n");
    for t in load("tags.json").as_array().unwrap() {
        let _ = writeln!(
            out,
            "TagEntry{{name:{},deprecated:{},deprecation_reason:{},parameters:{}}},",
            lit(t.get("name").and_then(Value::as_str).unwrap()),
            truthy(t.get("deprecated")),
            opt_str(t.get("deprecation_reason")),
            parameters(t.get("parameters")),
        );
    }
    out.push_str("];\n");

    let translations = load("shopify_system_translations.json");
    let mut keys: Vec<&str> = translations.as_object().unwrap().keys().collect();
    keys.sort_unstable();
    let _ = writeln!(out, "pub static SYSTEM_TRANSLATION_KEYS: &[&str] = &[");
    for k in keys {
        let _ = writeln!(out, "{},", lit(k));
    }
    out.push_str("];\n");

    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("docs_gen.rs");
    std::fs::write(dest, out).unwrap();
}
