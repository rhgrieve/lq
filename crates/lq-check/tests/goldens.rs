//! Replays `tests/goldens/*.jsonl`, recorded from upstream's specs, with upstream's bugs
//! reproduced. Records for checks not ported yet count as skipped and are listed; a record
//! that replays and differs fails. Run one file with `lq debug-check-golden <file> --upstream-bugs`.

use lq_check::golden::replay_file;
use std::path::Path;

/// The parser and the walk recurse on nesting depth.
const STACK: usize = 1 << 30;

fn golden_files() -> Vec<std::path::PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/goldens");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("tests/goldens")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    files
}

fn run() {
    let files = golden_files();
    let (mut total, mut passed, mut skipped, mut unreplayable) = (0, 0, 0, 0);
    let mut failures = Vec::new();
    for file in files {
        let s = replay_file(&file, lq_check::Divergences::NONE).expect("readable golden file");
        total += s.total;
        passed += s.passed;
        skipped += s.not_implemented;
        unreplayable += s.unreplayable;
        for (test, diff) in s.failures {
            failures.push(format!("{}: {test}\n  {diff}", s.check));
        }
    }
    println!("goldens: total={total} passed={passed} skipped={skipped} unreplayable={unreplayable}");
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn goldens_replay() {
    std::thread::Builder::new().stack_size(STACK).spawn(run).unwrap().join().unwrap();
}

/// Upstream's specs that default mode contradicts, by design. See `DIVERGENCES.md`.
const CONTRADICTED_BY_FIXES: [&str; 4] = [
    "LiquidSyntaxError > block and partial parser errors > reports LiquidSyntaxError for unclosed 'block' tags inside liquid blocks",
    "LiquidSyntaxError > block and partial parser errors > reports LiquidSyntaxError for unclosed 'partial' tags inside liquid blocks",
    "Module: MissingContentForArguments > missing required arguments > should report missing required arguments",
    "Module: MissingRenderSnippetArguments > missing required arguments > should report missing required arguments",
];

fn run_default_mode() {
    let mut failures = Vec::new();
    for file in golden_files() {
        let s = replay_file(&file, lq_check::Divergences::ALL).expect("readable golden file");
        failures.extend(s.failures.into_iter().map(|(test, _)| test));
    }
    failures.sort();
    failures.dedup();
    assert_eq!(failures, CONTRADICTED_BY_FIXES);
}

#[test]
fn only_known_specs_differ_with_fixes_on() {
    std::thread::Builder::new().stack_size(STACK).spawn(run_default_mode).unwrap().join().unwrap();
}
