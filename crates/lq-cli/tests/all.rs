//! `lq all`: check, then fmt, through the real binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CLEAN: &str = "<div>{{ a }}</div>\n";
const MESSY: &str = "<div>{{a}}</div>\n";
/// An error for `LiquidHTMLSyntaxError`, that fmt still reads as unformatted, as Prettier does.
const BROKEN: &str = "{% render c %\n";
/// Unformatted, with a deprecated filter that `--auto-correct` replaces.
const FIXABLE: &str = "<div>{{'#000'|hex_to_rgba}}</div>\n";

/// A theme whose one snippet has these contents, with only the checks these cases need turned on.
fn theme(name: &str, snippet: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("all-{name}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("snippets")).unwrap();
    fs::write(
        root.join(".theme-check.yml"),
        "extends: theme-check:nothing\nLiquidHTMLSyntaxError:\n  enabled: true\nDeprecatedFilter:\n  enabled: true\n",
    )
    .unwrap();
    fs::write(root.join("snippets/a.liquid"), snippet).unwrap();
    root
}

fn lq(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lq")).args(args).current_dir(cwd).output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn a_clean_theme_passes() {
    let root = theme("clean", CLEAN);
    let o = lq(&root, &["all"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(stderr(&o).contains("All matched files use Prettier code style!"), "{}", stderr(&o));
}

#[test]
fn a_lint_failure_fails_and_fmt_still_runs() {
    let root = theme("lint", CLEAN);
    fs::write(root.join("snippets/b.liquid"), BROKEN).unwrap();
    let o = lq(&root, &["all", "--output", "short"]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stdout(&o).contains("LiquidHTMLSyntaxError"), "{}", stdout(&o));
    assert!(stderr(&o).contains("[warn] snippets/b.liquid"), "{}", stderr(&o));
}

#[test]
fn an_error_from_fmt_outranks_offenses_from_check() {
    let root = theme("worst", BROKEN);
    fs::write(root.join(".prettierrc"), "{").unwrap();
    let o = lq(&root, &["all", "--output", "short"]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(stdout(&o).contains("LiquidHTMLSyntaxError"), "{}", stdout(&o));
    assert!(stderr(&o).contains("[error]"), "{}", stderr(&o));
}

#[test]
fn check_failing_alone_exits_1_and_fmt_still_reports() {
    let root = theme("check-only", CLEAN);
    fs::write(root.join(".theme-check.yml"), "extends: theme-check:nothing\nDeprecatedFilter:\n  enabled: true\n").unwrap();
    fs::write(root.join("snippets/a.liquid"), "<div>{{ '#000' | hex_to_rgba }}</div>\n").unwrap();
    let o = lq(&root, &["all", "--fail-level", "warning", "--output", "short"]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stdout(&o).contains("DeprecatedFilter"), "{}", stdout(&o));
    assert!(stderr(&o).contains("All matched files use Prettier code style!"), "{}", stderr(&o));
}

#[test]
fn a_format_failure_alone_exits_1() {
    let root = theme("fmt", MESSY);
    let o = lq(&root, &["all"]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains("[warn] snippets/a.liquid"), "{}", stderr(&o));
    assert_eq!(fs::read_to_string(root.join("snippets/a.liquid")).unwrap(), MESSY);
}

#[test]
fn stdout_holds_only_checks_report() {
    let root = theme("json", FIXABLE);
    let alone = lq(&root, &["check", "--output", "json"]);
    let both = lq(&root, &["all", "--output", "json"]);
    assert_eq!(stdout(&both), stdout(&alone));
    assert_eq!(both.status.code(), Some(1), "{}", stderr(&both));
    assert!(stderr(&both).contains("Checking formatting..."), "{}", stderr(&both));
}

#[test]
fn write_fixes_then_formats_and_a_rerun_is_clean() {
    let root = theme("write", FIXABLE);
    let o = lq(&root, &["all", "--write"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(stdout(&o), "");
    let fixed = fs::read_to_string(root.join("snippets/a.liquid")).unwrap();
    assert!(!fixed.contains("hex_to_rgba"), "{fixed}");
    assert!(fixed.contains("{{ "), "{fixed}");
    let again = lq(&root, &["all", "--fail-level", "warning"]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
}

#[test]
fn write_lists_only_the_files_it_changed() {
    let root = theme("write-list", MESSY);
    fs::write(root.join("snippets/b.liquid"), CLEAN).unwrap();
    let o = lq(&root, &["all", "-w"]);
    assert!(stderr(&o).contains("snippets/a.liquid"), "{}", stderr(&o));
    assert!(!stderr(&o).contains("snippets/b.liquid"), "{}", stderr(&o));
}

#[test]
fn a_missing_path_exits_2_before_fmt() {
    let root = theme("missing", MESSY);
    let o = lq(&root, &["all", "snippets/nope.liquid"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("doesn't exist"), "{}", stderr(&o));
    assert!(!stderr(&o).contains("Checking formatting"), "{}", stderr(&o));
}

#[test]
fn file_arguments_narrow_both_and_other_files_are_skipped() {
    let root = theme("files", MESSY);
    fs::write(root.join("snippets/b.liquid"), CLEAN).unwrap();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(root.join("config/settings_schema.json"), "[]\n").unwrap();
    let o = lq(&root, &["all", "snippets/b.liquid", "config/settings_schema.json"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(!stderr(&o).contains("[error]"), "{}", stderr(&o));
    assert!(!stderr(&o).contains("snippets/a.liquid"), "{}", stderr(&o));
}

#[test]
fn path_moves_fmt_to_the_theme_root_too() {
    let root = theme("path", MESSY);
    fs::write(root.join("snippets/skipped.liquid"), MESSY).unwrap();
    fs::write(root.join(".prettierignore"), "snippets/skipped.liquid\n").unwrap();
    let elsewhere = root.parent().unwrap();
    let o = lq(elsewhere, &["all", "--path", root.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains("[warn] snippets/a.liquid"), "{}", stderr(&o));
    assert!(!stderr(&o).contains("skipped.liquid"), "{}", stderr(&o));
}

#[test]
fn flags_that_do_not_fit_exit_2_and_touch_nothing() {
    let root = theme("rejected", MESSY);
    for flag in ["--auto-correct", "-a", "--init", "--list", "--print", "--version", "-v", "--nope"] {
        let o = lq(&root, &["all", flag]);
        assert_eq!(o.status.code(), Some(2), "{flag}: {}", stderr(&o));
    }
    assert_eq!(fs::read_to_string(root.join("snippets/a.liquid")).unwrap(), MESSY);
}

#[test]
fn check_keeps_its_exit_code_now_that_it_returns() {
    let root = theme("check-code", BROKEN);
    assert_eq!(lq(&root, &["check"]).status.code(), Some(1));
    fs::write(root.join("snippets/a.liquid"), CLEAN).unwrap();
    assert_eq!(lq(&root, &["check"]).status.code(), Some(0));
}
