//! Upstream's theme graph specs (`specs/graph/*.yml`), run against real files on disk.

use lq_check::deps::NodeDependencies;
use lq_check::graph::{build_theme_graph, ReferenceType};
use lq_check::registry::CONFIG_REGISTRY;
use lq_check::to_source_code;
use lq_json::{uri, Value};
use lq_theme::config::load_config;
use lq_theme::discovery::get_theme_files;
use lq_theme::divergence::Divergences;
use std::fs;
use std::path::{Path, PathBuf};

const CONTEXTUAL: &str = include_str!("specs/graph-contextual.yml");

/// One edge, as a spec writes it: `source target type [line, column] text`.
type Edge = (String, String, &'static str, Option<(u32, u32)>, Option<String>);

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

fn line_column(source: &str, offset: u32) -> (u32, u32) {
    let before = &source[..offset as usize];
    let line = before.matches('\n').count() as u32;
    let column = before.rsplit('\n').next().unwrap().len() as u32;
    (line, column)
}

struct Graph {
    edges: Vec<Edge>,
    entry_points: Vec<String>,
}

fn build(root: &Path, divergences: Divergences) -> Result<Graph, String> {
    let root = root.to_str().unwrap();
    let mut config = load_config(None, root, CONFIG_REGISTRY).map_err(|e| e.message)?;
    config.divergences = divergences;
    let sources: Vec<(String, String)> =
        get_theme_files(&config).into_iter().map(|p| (uri::file_uri(&p), fs::read_to_string(&p).unwrap())).collect();
    let theme: lq_check::Theme<'_> = sources.iter().map(|(u, s)| to_source_code(u, s)).collect();
    let deps = NodeDependencies::new(&theme, &config, root);
    let graph = build_theme_graph(&config.root_uri, &deps, &theme, config.context, config.divergences)?;
    let rel = |u: &str| uri::relative(u, &config.root_uri);
    let mut edges = Vec::new();
    for module in graph.modules.values() {
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
    let mut entry_points: Vec<String> = graph.entry_points.iter().map(|u| rel(u)).collect();
    entry_points.sort();
    Ok(Graph { edges, entry_points })
}

fn expected_edges(case: &Value) -> Vec<Edge> {
    let mut edges: Vec<Edge> = case
        .get("edges")
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
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

#[test]
fn contextual_templates_and_section_groups() {
    let spec = lq_theme::yaml::parse(CONTEXTUAL).unwrap();
    let cases = spec.get("cases").and_then(Value::as_array).unwrap();
    assert_eq!(cases.len(), 6);
    for case in cases {
        let name = str_field(case, "name").unwrap();
        let root = theme(name, case.get("files").unwrap());
        let graph = build(&root, Divergences::ALL).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(graph.edges, expected_edges(case), "{name}");
        if let Some(want) = case.get("entryPoints").and_then(Value::as_array) {
            let mut want: Vec<String> = want.iter().map(|v| v.as_str().unwrap().to_string()).collect();
            want.sort();
            assert_eq!(graph.entry_points, want, "{name}");
        }
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn upstream_bugs_still_fail_on_an_inherited_type() {
    let spec = lq_theme::yaml::parse(CONTEXTUAL).unwrap();
    let case = &spec.get("cases").and_then(Value::as_array).unwrap()[0];
    let root = theme("upstream-bugs", case.get("files").unwrap());
    assert!(build(&root, Divergences::NONE).is_err());
    fs::remove_dir_all(root).unwrap();
}
