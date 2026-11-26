//! Upstream's check specs (`specs/checks` in Shopify/theme-tools), run against lq.
//!
//!     LQ_UPSTREAM_SPECS=<theme-tools>/specs cargo test -p lq-check --test upstream_specs -- --ignored --nocapture
//!
//! `LQ_SPECS_UPSTREAM_BUGS=1` runs with every divergence off. It reports and never fails:
//! the specs follow upstream's `main`, and lq targets a release.

use lq_check::upstream_specs::{run_case, CaseOutcome};
use lq_json::Value;
use lq_theme::divergence::Divergences;
use std::collections::BTreeMap;
use std::path::Path;

#[test]
#[ignore = "needs LQ_UPSTREAM_SPECS"]
fn upstream_check_specs() {
    // Deeply nested cases need more than a test thread's stack, as the CLI's threads have.
    std::thread::Builder::new().stack_size(256 << 20).spawn(run).unwrap().join().unwrap();
}

fn run() {
    let Ok(root) = std::env::var("LQ_UPSTREAM_SPECS") else { return };
    let dir = Path::new(&root).join("checks");
    let divergences = if std::env::var_os("LQ_SPECS_UPSTREAM_BUGS").is_some() { Divergences::NONE } else { Divergences::ALL };
    let docsets = |name: &str| {
        let text = std::fs::read_to_string(dir.join("docsets").join(format!("{name}.yml"))).ok()?;
        lq_theme::yaml::parse(&text).ok()
    };
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    files.retain(|p| p.extension().is_some_and(|e| e == "yml"));
    files.sort();
    let (mut passed, mut failed, mut unsupported) = (0, 0, BTreeMap::<String, usize>::new());
    let mut by_spec = BTreeMap::<String, (usize, usize)>::new();
    let mut statuses = lq_json::Object::new();
    for file in &files {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        let spec = match lq_theme::yaml::parse(&std::fs::read_to_string(file).unwrap()) {
            Ok(s) => s,
            Err(e) => {
                println!("YAML {name}: {e:?}");
                continue;
            }
        };
        let check = spec.get("check").and_then(Value::as_str);
        let severity = spec.get("severity").and_then(Value::as_str);
        for case in spec.get("cases").and_then(Value::as_array).unwrap_or(&[]) {
            let case_name = case.get("name").and_then(Value::as_str).unwrap_or("");
            let entry = by_spec.entry(name.clone()).or_default();
            let outcome = run_case(check, severity, case, &docsets, divergences);
            let status = match &outcome {
                CaseOutcome::Passed => "pass".to_string(),
                CaseOutcome::Failed(why) => format!("fail: {why}"),
                CaseOutcome::Unsupported(why) => format!("unsupported: {why}"),
            };
            statuses.insert(format!("{name}: {case_name}"), Value::String(status));
            match outcome {
                CaseOutcome::Passed => {
                    passed += 1;
                    entry.0 += 1;
                }
                CaseOutcome::Failed(why) => {
                    failed += 1;
                    entry.1 += 1;
                    println!("FAIL {name}: {case_name}\n      {why}");
                }
                CaseOutcome::Unsupported(why) => *unsupported.entry(why).or_default() += 1,
            }
        }
    }
    if let Some(out) = std::env::var_os("LQ_SPECS_OUT") {
        std::fs::write(out, Value::Object(statuses).to_json()).unwrap();
    }
    println!("\nby spec (passed, failed):");
    for (k, (p, f)) in &by_spec {
        if *f > 0 {
            println!("  {k}: {p} passed, {f} failed");
        }
    }
    println!("\nunsupported: {unsupported:?}");
    println!("passed {passed}, failed {failed}, unsupported {}", unsupported.values().sum::<usize>());
}
