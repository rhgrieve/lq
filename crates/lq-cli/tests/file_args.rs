//! What `lq check <files>` reports, and what it does when a path is wrong.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

const BROKEN: &str = "<div>\n  {% for a in b %}\n    {% render c %\n  {% endfor %}\n</div>\n";

fn theme(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("lq-file-args-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("snippets")).unwrap();
    fs::write(root.join(".theme-check.yml"), "extends: theme-check:recommended\n").unwrap();
    fs::write(root.join("snippets/clean.liquid"), "ok\n").unwrap();
    fs::write(root.join("snippets/broken.liquid"), BROKEN).unwrap();
    root
}

fn lq(root: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lq")).args(args).current_dir(root).output().unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn a_clean_file_mentions_errors_elsewhere_without_failing() {
    let root = theme("hidden");
    let o = lq(&root, &["check", "snippets/clean.liquid"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(stderr(&o).contains("1 error in 1 other file not shown"), "{}", stderr(&o));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn naming_the_broken_file_reports_it_with_no_note() {
    let root = theme("named");
    let o = lq(&root, &["check", "snippets/broken.liquid"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("LiquidHTMLSyntaxError"), "{}", stderr(&o));
    assert!(!stderr(&o).contains("not shown"), "{}", stderr(&o));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_whole_theme_has_no_note() {
    let root = theme("whole");
    let o = lq(&root, &["check"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(!stderr(&o).contains("not shown"), "{}", stderr(&o));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_path_that_does_not_exist_fails() {
    let root = theme("missing");
    let o = lq(&root, &["check", "nope.liquid"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("nope.liquid doesn't exist"), "{}", stderr(&o));
    // One good path doesn't excuse a bad one.
    let o = lq(&root, &["check", "snippets/clean.liquid", "nope.liquid"]);
    assert_eq!(o.status.code(), Some(2));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn json_stdout_stays_valid_with_the_note() {
    let root = theme("json");
    let o = lq(&root, &["check", "snippets/clean.liquid", "--output", "json"]);
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(stdout.trim_start().starts_with('{') && stdout.trim_end().ends_with('}'), "{stdout}");
    assert!(!stdout.contains("not shown"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}
