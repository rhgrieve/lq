//! What `lq fmt --check` tells someone to do about unformatted files.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MESSY: &str = "<div   class=\"a\">{{a|b}}</div>\n";

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("lq-fmt-check-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.liquid"), MESSY).unwrap();
    fs::write(root.join("b.liquid"), MESSY).unwrap();
    root
}

fn lq(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lq")).args(args).current_dir(root).output().unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn the_hint_names_lq_for_one_file() {
    let root = project("one");
    let o = lq(&root, &["fmt", "--check", "a.liquid"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        stderr(&o),
        "[warn] a.liquid\n[warn] Code style issues found in the above file. Run `lq fmt` with --write to fix.\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_hint_names_lq_for_several_files() {
    let root = project("several");
    let o = lq(&root, &["fmt", "--check", "a.liquid", "b.liquid"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).ends_with("[warn] Code style issues found in 2 files. Run `lq fmt` with --write to fix.\n"),
        "{}",
        stderr(&o)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn following_the_hint_fixes_the_file() {
    let root = project("follow");
    assert_eq!(lq(&root, &["fmt", "--write", "a.liquid"]).status.code(), Some(0));
    assert_eq!(lq(&root, &["fmt", "--check", "a.liquid"]).status.code(), Some(0));
    fs::remove_dir_all(root).unwrap();
}
