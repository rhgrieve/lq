//! What `lq fmt --check` tells someone to do about unformatted files.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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

/// Files that don't render HTML, in which a reflow changes what they render.
const TEXT: [(&str, &str); 3] = [
    ("templates/robots.txt.liquid", "{%- if a -%}\nUser-agent: *\nDisallow: /\n{%- endif -%}\n"),
    ("assets/a.js.liquid", "var a = {{a|json}}; // note\nvar b = 2;\n"),
    ("assets/a.css.liquid", ".a{color:{{c}}}\n\n\n.b{top:0}"),
];

fn text_project(name: &str) -> PathBuf {
    let root = project(name);
    for (path, source) in TEXT {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }
    root
}

/// `lq` with `input` on stdin.
fn lq_stdin(root: &Path, args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lq"))
        .args(args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn text_files_pass_check_and_print_as_they_are() {
    let root = text_project("text-check");
    for (path, source) in TEXT {
        assert_eq!(lq(&root, &["fmt", "--check", path]).status.code(), Some(0), "{path}");
        assert_eq!(lq(&root, &["fmt", "--list-different", path]).stdout, b"", "{path}");
        assert_eq!(lq(&root, &["fmt", path]).stdout, source.as_bytes(), "{path}");
        let o = lq_stdin(&root, &["fmt", "--stdin-filepath", path], source);
        assert_eq!((o.status.code(), o.stdout.as_slice()), (Some(0), source.as_bytes()), "{path}");
        let o = lq_stdin(&root, &["fmt", "--check", "--stdin-filepath", path], source);
        assert_eq!((o.status.code(), o.stdout.as_slice()), (Some(0), &b""[..]), "{path}");
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn write_and_all_leave_text_files_alone() {
    for args in [&["fmt", "--write", "**/*.liquid"][..], &["all", "--write"]] {
        let root = text_project(&format!("text-{}", args[0]));
        lq(&root, args);
        for (path, source) in TEXT {
            assert_eq!(fs::read_to_string(root.join(path)).unwrap(), source, "{args:?} {path}");
        }
        assert_ne!(fs::read_to_string(root.join("a.liquid")).unwrap(), MESSY, "{args:?}");
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn upstream_bugs_formats_text_files_as_html() {
    let root = text_project("text-upstream");
    let (path, source) = TEXT[1];
    let reflowed = "var a = {{ a | json }}; // note var b = 2;\n";
    assert_eq!(lq(&root, &["fmt", "--upstream-bugs", path]).stdout, reflowed.as_bytes());
    let o = lq_stdin(&root, &["fmt", "--upstream-bugs", "--stdin-filepath", path], source);
    assert_eq!(o.stdout, reflowed.as_bytes());
    let o = lq(&root, &["all", "--upstream-bugs"]);
    assert!(stderr(&o).contains("[warn] assets/a.js.liquid"), "{}", stderr(&o));
    fs::remove_dir_all(root).unwrap();
}
