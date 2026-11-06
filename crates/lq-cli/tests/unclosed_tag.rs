//! A tag missing its `%}` makes the parser fail on later markup. The offense stays upstream's,
//! on that later markup, and a note in the reports for people points at the tag.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

const BROKEN: &str = "<div>\n  {% for a in b %}\n    {% render c %\n  {% endfor %}\n</div>\n";
const UPSTREAM: &str = "Attempting to close HtmlElement 'div' before it was opened";
const NOTE: &str = "this tag isn't closed, so it runs on to the `%}` on line 4";

fn theme(name: &str, source: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("lq-unclosed-tag-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("snippets")).unwrap();
    fs::write(root.join(".theme-check.yml"), "extends: theme-check:recommended\n").unwrap();
    fs::write(root.join("snippets/a.liquid"), source).unwrap();
    root
}

fn lq(root: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lq"))
        .arg("check")
        .args(args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        .output()
        .unwrap()
}

fn all_output(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

#[test]
fn json_stays_upstreams() {
    let root = theme("json", BROKEN);
    for args in [&["--output", "json"][..], &["--output", "json", "--upstream-bugs"][..]] {
        let out = all_output(&lq(&root, args));
        assert!(out.contains(UPSTREAM), "{out}");
        assert!(out.contains("\"startRow\":5"), "{out}");
        assert!(!out.contains("isn't closed"), "{out}");
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_text_report_points_at_the_tag() {
    let root = theme("text", BROKEN);
    let o = lq(&root, &[]);
    assert_eq!(o.status.code(), Some(1));
    let out = all_output(&o);
    assert!(out.contains(UPSTREAM), "{out}");
    assert!(out.contains(NOTE), "{out}");
    assert!(out.contains("snippets/a.liquid:3:5"), "{out}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn json_rich_carries_the_note() {
    let root = theme("rich", BROKEN);
    let out = all_output(&lq(&root, &["--output", "json-rich"]));
    assert!(out.contains(NOTE), "{out}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn upstream_bugs_has_upstreams_report() {
    let root = theme("bugs", BROKEN);
    let out = all_output(&lq(&root, &["--upstream-bugs"]));
    assert!(out.contains(UPSTREAM), "{out}");
    assert!(!out.contains("isn't closed"), "{out}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_stray_opener_that_is_not_an_unclosed_tag_gets_no_note() {
    for (name, source) in [
        ("string", "{{ \"{%\" }}\n</div>\n"),
        ("comment", "{% comment %} {{ {% endcomment %}\n</div>\n"),
        ("raw", "{% raw %} {% {% endraw %}\n</div>\n"),
        ("none", "ok\n</div>\n"),
    ] {
        let root = theme(name, source);
        let out = all_output(&lq(&root, &[]));
        assert!(out.contains(UPSTREAM), "{name}: {out}");
        assert!(!out.contains("isn't closed"), "{name}: {out}");
        fs::remove_dir_all(root).unwrap();
    }
}
