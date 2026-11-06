//! `--only`, `--skip` and `--severity`, through the real binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MIXED: &str = "{% assign unused = 1 %}\n{{ 'a' | hex_to_rgba }}\n<a href=\"/cart\">x</a>\n";

/// One error, and warnings from `OrphanedSnippet`, `UnusedAssign`, `DeprecatedFilter` and `HardcodedRoutes`.
fn theme(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("filter-{name}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("snippets")).unwrap();
    fs::write(root.join(".theme-check.yml"), "extends: theme-check:recommended\n").unwrap();
    fs::write(root.join("snippets/broken.liquid"), "{% render c %\n").unwrap();
    fs::write(root.join("snippets/mixed.liquid"), MIXED).unwrap();
    root
}

fn lq(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lq")).arg("check").args(args).current_dir(root).output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// The check of each line of `--output short`.
fn checks(o: &Output) -> Vec<String> {
    let mut checks: Vec<String> = stdout(o)
        .lines()
        .map(|line| line.split(": ").nth(1).unwrap().split(' ').nth(1).unwrap().to_string())
        .collect();
    checks.sort();
    checks.dedup();
    checks
}

fn short(root: &Path, args: &[&str]) -> Output {
    lq(root, &[&["--output", "short"], args].concat())
}

#[test]
fn only_reports_one_check_and_leaves_the_rest_out_of_the_exit_code() {
    let root = theme("only");
    let o = short(&root, &["--only", "UnusedAssign"]);
    assert_eq!(checks(&o), ["UnusedAssign"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(stderr(&o).contains("offenses not shown because of --only, including 1 error."), "{}", stderr(&o));
}

#[test]
fn selecting_a_failing_check_still_fails() {
    let root = theme("fails");
    let o = short(&root, &["--only", "liquidhtml"]);
    assert_eq!(checks(&o), ["LiquidHTMLSyntaxError"]);
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn commas_and_repeats_add_up() {
    let root = theme("add");
    let expected = ["HardcodedRoutes", "UnusedAssign"];
    assert_eq!(checks(&short(&root, &["--only", "UnusedAssign,HardcodedRoutes"])), expected);
    assert_eq!(checks(&short(&root, &["--only=UnusedAssign", "--only=HardcodedRoutes"])), expected);
}

#[test]
fn skip_wins_over_only() {
    let root = theme("skip");
    let o = short(&root, &["--only", "UnusedAssign,HardcodedRoutes", "--skip", "unused"]);
    assert_eq!(checks(&o), ["HardcodedRoutes"]);
    let o = short(&root, &["--skip", "LiquidHTMLSyntaxError"]);
    assert!(!checks(&o).contains(&"LiquidHTMLSyntaxError".to_string()));
    assert_eq!(o.status.code(), Some(0));
}

#[test]
fn severity_is_a_minimum_and_applies_before_the_fail_level() {
    let root = theme("severity");
    assert_eq!(checks(&short(&root, &["--severity", "error"])), ["LiquidHTMLSyntaxError"]);
    assert!(checks(&short(&root, &["--severity", "warning"])).contains(&"UnusedAssign".to_string()));
    let o = short(&root, &["--severity", "error", "--skip", "LiquidHTMLSyntaxError", "--fail-level", "info"]);
    assert_eq!(stdout(&o), "");
    assert_eq!(o.status.code(), Some(0));
}

#[test]
fn filters_combine_with_file_arguments() {
    let root = theme("files");
    let o = short(&root, &["--only", "OrphanedSnippet", "snippets/mixed.liquid"]);
    assert_eq!(stdout(&o).lines().count(), 1, "{}", stdout(&o));
    assert!(stdout(&o).starts_with("snippets/mixed.liquid"), "{}", stdout(&o));
}

#[test]
fn a_typo_exits_2_with_a_suggestion() {
    let root = theme("typo");
    for flag in ["--only", "--skip"] {
        let o = lq(&root, &[flag, "OrphanedSnipet"]);
        assert_eq!(o.status.code(), Some(2));
        assert!(stderr(&o).contains(&format!("no check matches {flag} OrphanedSnipet")), "{}", stderr(&o));
        assert!(stderr(&o).contains("did you mean 'OrphanedSnippet'?"), "{}", stderr(&o));
    }
}

#[test]
fn bad_values_are_usage_errors() {
    let root = theme("usage");
    assert_eq!(lq(&root, &["--severity", "loud"]).status.code(), Some(2));
    assert_eq!(lq(&root, &["--only="]).status.code(), Some(2));
    assert_eq!(lq(&root, &["--only", "a,,b"]).status.code(), Some(2));
}

#[test]
fn a_check_turned_off_in_the_config_gets_a_note() {
    let root = theme("off");
    fs::write(root.join(".theme-check.yml"), "extends: theme-check:recommended\nUnusedAssign:\n  enabled: false\n").unwrap();
    let o = short(&root, &["--only", "UnusedAssign"]);
    assert_eq!(stdout(&o), "");
    assert!(stderr(&o).contains("UnusedAssign is turned off in the config"), "{}", stderr(&o));
}

/// The parity gates compare this format, so filtering must only ever take offenses out of it.
#[test]
fn json_is_the_same_shape_with_fewer_offenses() {
    let root = theme("json");
    let whole = stdout(&lq(&root, &["--output", "json"]));
    let filtered = stdout(&lq(&root, &["--output", "json", "--only", "LiquidHTMLSyntaxError"]));
    assert!(whole.contains("UnusedAssign"), "{whole}");
    assert!(!filtered.contains("UnusedAssign"), "{filtered}");
    assert!(filtered.contains("LiquidHTMLSyntaxError"), "{filtered}");
}

#[test]
fn no_filter_means_no_note() {
    let root = theme("none");
    let o = lq(&root, &["--color", "never"]);
    assert!(!stderr(&o).contains("not shown"), "{}", stderr(&o));
}

#[test]
fn auto_correct_only_applies_what_the_filter_selects() {
    let root = theme("fix");
    let o = lq(&root, &["--only", "DeprecatedFilter", "--auto-correct"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let fixed = fs::read_to_string(root.join("snippets/mixed.liquid")).unwrap();
    assert!(fixed.contains("{% assign unused = 1 %}"), "{fixed}");
    assert!(!fixed.contains("hex_to_rgba"), "{fixed}");
}

#[test]
fn list_shows_what_a_filter_selects() {
    let root = theme("list");
    let listed = stdout(&lq(&root, &["--config", "theme-check:all", "--list", "--only", "Asset*", "--skip", "AssetPreload"]));
    assert!(listed.contains("AssetSizeCSS:"), "{listed}");
    assert!(!listed.contains("AssetPreload:"), "{listed}");
    assert!(!listed.contains("RemoteAsset:"), "{listed}");
}
