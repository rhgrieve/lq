//! Upstream's theme graph specs (`specs/upstream/graph`), run against real files on disk, the way
//! `packages/theme-graph/src/specs.spec.ts` does. `LQ_UPSTREAM_SPECS=<theme-tools>/specs` runs
//! another copy.
//!
//! `shopify theme check` builds its graph without custom element definitions
//! (`getWebComponentDefinitionReference: () => undefined`), so a case's edges to a custom
//! element's script, and its module list when it has such an edge, aren't expected here.

use lq_check::deps::NodeDependencies;
use lq_check::graph::{build_theme_graph, ModuleKind, ReferenceType, ThemeModule};
use lq_check::registry::CONFIG_REGISTRY;
use lq_check::to_source_code;
use lq_json::{uri, Value};
use lq_theme::config::load_config;
use lq_theme::discovery::get_theme_files;
use std::fs;
use std::path::{Path, PathBuf};

/// One edge, as a spec writes it: `source target type [line, column] text`.
type Edge = (String, String, &'static str, Option<(u32, u32)>, Option<String>);
/// One module: `file type kind exists`.
type Module = (String, String, String, bool);

fn specs_dir() -> PathBuf {
    match std::env::var_os("LQ_UPSTREAM_SPECS") {
        Some(d) => PathBuf::from(d).join("graph"),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../specs/upstream/graph"),
    }
}

fn str_field<'v>(v: &'v Value, k: &str) -> Option<&'v str> {
    v.get(k).and_then(Value::as_str)
}

fn theme(case: &str, files: &Value) -> PathBuf {
    let slug: String = case.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let root = std::env::temp_dir().join(format!("lq-graph-{}-{slug}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for (path, text) in files.as_object().unwrap().iter() {
        let file = root.join(path.to_string());
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text.as_str().unwrap()).unwrap();
    }
    root
}

/// `[line, character]` of a byte offset, in UTF-16 code units.
fn line_column(source: &str, offset: u32) -> (u32, u32) {
    let before = &source[..offset as usize];
    let line = before.matches('\n').count() as u32;
    let column = before.rsplit('\n').next().unwrap().encode_utf16().count() as u32;
    (line, column)
}

/// A module's `type` and `kind`, as the specs spell them.
fn type_and_kind(m: &ThemeModule) -> (&'static str, &'static str) {
    match m.kind {
        ModuleKind::Layout => ("Liquid", "layout"),
        ModuleKind::Section => ("Liquid", "section"),
        ModuleKind::Block => ("Liquid", "block"),
        ModuleKind::Snippet => ("Liquid", "snippet"),
        ModuleKind::LiquidTemplate => ("Liquid", "template"),
        ModuleKind::JsonTemplate => ("JSON", "template"),
        ModuleKind::SectionGroup => ("JSON", "section-group"),
        ModuleKind::Asset => {
            let name = m.uri.strip_suffix(".liquid").unwrap_or(&m.uri);
            let ty = match name.rsplit('.').next().unwrap_or("") {
                "js" => "JavaScript",
                "css" | "scss" => "CSS",
                "svg" => "SVG",
                _ => "Image",
            };
            (ty, "unused")
        }
    }
}

struct Graph {
    edges: Vec<Edge>,
    entry_points: Vec<String>,
    modules: Vec<Module>,
}

fn build(root: &Path, app: bool) -> Result<Graph, String> {
    let root = root.to_str().unwrap();
    let mut config = load_config(None, root, CONFIG_REGISTRY).map_err(|e| e.message)?;
    if app {
        config.context = lq_theme::config::Mode::App;
    }
    let sources: Vec<(String, String)> =
        get_theme_files(&config).into_iter().map(|p| (uri::file_uri(&p), fs::read_to_string(&p).unwrap())).collect();
    let theme: lq_check::Theme<'_> = sources.iter().map(|(u, s)| to_source_code(u, s)).collect();
    let deps = NodeDependencies::new(&theme, &config, root);
    let graph = build_theme_graph(&config.root_uri, &deps, &theme, config.context)?;
    let rel = |u: &str| uri::relative(u, &config.root_uri);
    let mut edges = Vec::new();
    let mut modules = Vec::new();
    for module in graph.modules.values() {
        let (ty, kind) = type_and_kind(module);
        modules.push((rel(&module.uri), ty.to_string(), kind.to_string(), module.exists == Some(true)));
        for r in &module.dependencies {
            let source = fs::read_to_string(uri::fs_path(&r.source.uri)).unwrap_or_default();
            let ty = match r.ty {
                ReferenceType::Direct => "direct",
                ReferenceType::Indirect => "indirect",
                ReferenceType::Preset => "preset",
            };
            let start = r.source.range.map(|(s, _)| line_column(&source, s));
            let text = r.source.range.map(|(s, e)| source[s as usize..e as usize].to_string());
            edges.push((rel(&r.source.uri), rel(&r.target.uri), ty, start, text));
        }
    }
    edges.sort();
    modules.sort();
    let mut entry_points: Vec<String> = graph.entry_points.iter().map(|u| rel(u)).collect();
    entry_points.sort();
    Ok(Graph { edges, entry_points, modules })
}

fn expected_edges(case: &Value) -> Vec<Edge> {
    let mut edges: Vec<Edge> = case
        .get("edges")
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter(|e| e.get("targetStart").is_none())
        .map(|e| {
            let start = e.get("start").and_then(Value::as_array).map(|p| {
                let n = |i: usize| p[i].as_f64().unwrap() as u32;
                (n(0), n(1))
            });
            let ty = match str_field(e, "type") {
                Some("indirect") => "indirect",
                Some("preset") => "preset",
                _ => "direct",
            };
            (
                str_field(e, "source").unwrap().to_string(),
                str_field(e, "target").unwrap().to_string(),
                ty,
                start,
                str_field(e, "text").map(String::from),
            )
        })
        .collect();
    edges.sort();
    edges
}

fn expected_modules(list: &[Value]) -> Vec<Module> {
    let mut modules: Vec<Module> = list
        .iter()
        .map(|m| {
            let s = |k: &str| str_field(m, k).unwrap_or("").to_string();
            (s("file"), s("type"), s("kind"), !matches!(m.get("exists"), Some(Value::Bool(false))))
        })
        .collect();
    modules.sort();
    modules
}

#[test]
fn upstream_graph_specs() {
    let dir = specs_dir();
    let mut files: Vec<_> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    files.retain(|p| p.extension().is_some_and(|e| e == "yml"));
    files.sort();
    let mut failures = Vec::new();
    let mut count = 0;
    for file in &files {
        let spec = lq_theme::yaml::parse(&fs::read_to_string(file).unwrap()).unwrap();
        for case in spec.get("cases").and_then(Value::as_array).unwrap() {
            count += 1;
            let name = format!("{}: {}", file.file_name().unwrap().to_string_lossy(), str_field(case, "name").unwrap());
            let root = theme(&name, case.get("files").unwrap());
            let app = str_field(case, "mode") == Some("app");
            let graph = match build(&root, app) {
                Ok(g) => g,
                Err(e) => {
                    failures.push(format!("{name}: the graph failed: {e}"));
                    continue;
                }
            };
            let custom_elements = case
                .get("edges")
                .and_then(Value::as_array)
                .is_some_and(|e| e.iter().any(|e| e.get("targetStart").is_some()));
            if graph.edges != expected_edges(case) {
                failures.push(format!("{name}: edges\n    want {:?}\n    got  {:?}", expected_edges(case), graph.edges));
            }
            if let Some(want) = case.get("entryPoints").and_then(Value::as_array) {
                let mut want: Vec<String> = want.iter().map(|v| v.as_str().unwrap().to_string()).collect();
                want.sort();
                if graph.entry_points != want {
                    failures.push(format!("{name}: entry points\n    want {want:?}\n    got  {:?}", graph.entry_points));
                }
            }
            if let Some(want) = case.get("modules").and_then(Value::as_array).filter(|_| !custom_elements) {
                if graph.modules != expected_modules(want) {
                    failures.push(format!("{name}: modules\n    want {:?}\n    got  {:?}", expected_modules(want), graph.modules));
                }
            }
            fs::remove_dir_all(root).unwrap();
        }
    }
    assert!(count > 0, "no graph specs in {}", dir.display());
    assert!(failures.is_empty(), "{} of {count} graph cases failed:\n{}", failures.len(), failures.join("\n"));
}
