//! The reports of `lq check`, run through the real binary on a fixture theme. `UPDATE_SNAPSHOTS=1
//! cargo test -p lq-cli --test report` rewrites the snapshots after a deliberate change.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const LQ: &str = env!("CARGO_BIN_EXE_lq");

/// Every layout case the renderer has to get right. Written here, not checked in as files,
/// because control characters and CRLF don't survive editors and formatters.
fn fixture() -> PathBuf {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(write_fixture).clone()
}

fn write_fixture() -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("report-theme");
    let _ = std::fs::remove_dir_all(&root);
    let long = "x".repeat(300);
    let files: &[(&str, String)] = &[
        (".theme-check.yml", "extends: theme-check:recommended\n".into()),
        ("layout/theme.liquid", "<html><body>{{ content_for_header }}{{ content_for_layout }}</body></html>\n".into()),
        ("locales/en.default.json", r#"{"greeting":{"hello":"Hi"},"only":{"a":"x","b":"y"}}"#.into()),
        ("locales/fr.json", "{\n  \"greeting\": {\n    \"hello\": \"Salut\"\n  },\n  \"grreting\": \"x\",\n  \"only\": {\n    \"a\": \"x\"\n  }\n}\n".into()),
        ("locales/de.json", "/* generated */\n{\"greeting\":{\"hello\":\"Hallo\"}}\n".into()),
        ("snippets/card.liquid", "{% doc %}\n  @param {string} title - The title\n  @param {string} [subtitle] - The subtitle\n{% enddoc %}\n{{ title }}{{ subtitle }}\n".into()),
        ("snippets/orphan.liquid", "<p>nobody renders me</p>\n<p>at all</p>\n<p>really</p>\n".into()),
        (
            "sections/main.liquid",
            format!(
                "{{% render 'crad' %}}\n\
                 {{% render 'nothing-like-it' %}}\n\
                 {{% render 'nothing-like-it' %}}\n\
                 {{% render 'card', title: 'a', subtitel: 'b' %}}\n\
                 {{% render 'card' %}}\n\
                 {{{{ 'a' | upcas }}}}\n\
                 {{{{ produt.title }}}}\n\
                 {{{{ 'greeting.helo' | t }}}}\n\
                 \t\t{{{{ tabbed_thing }}}}\n\
                 <p>日本語のテキスト {{{{ wide_thing }}}}</p>\n\
                 <p class=\"{long}\">{{{{ long_thing }}}}</p>\n\
                 {{{{ 'esc\u{1b}[31mape.key' | t }}}}\n\
                 {{% if a %}}<div>{{% endif %}}\n\
                 {{% assign unused = 1 %}}\n\
                 {{% assign x = 'a' | img_url %}}{{{{ x }}}}\n"
            ),
        ),
        ("sections/crlf.liquid", "<p>\r\n{{ crlf_thing }}\r\n</p>\r\n".into()),
    ];
    for (path, text) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    root
}

fn run_in(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(LQ);
    command.arg("check").args(args).current_dir(root);
    for name in ["NO_COLOR", "FORCE_COLOR", "CLICOLOR_FORCE", "COLUMNS"] {
        command.env_remove(name);
    }
    command.envs(env.iter().copied());
    command.output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Takes the timing out, which is all that varies between runs.
fn stable(report: &str) -> String {
    let mut out = String::new();
    for line in report.lines() {
        match line.rfind(" checked in ") {
            Some(i) => {
                out.push_str(&line[..i]);
                out.push_str(" checked in <time>");
                if let Some(rest) = line[i..].find(',') {
                    out.push_str(&line[i + rest..]);
                }
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

fn assert_snapshot(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots").join(name);
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(expected == actual, "{name} differs. UPDATE_SNAPSHOTS=1 rewrites it. Got:\n{actual}");
}

#[test]
fn snapshots() {
    let root = fixture();
    let plain = run_in(&root, &["--color", "never"], &[("COLUMNS", "80")]);
    assert_snapshot("plain-80.txt", &stable(&text(&plain.stderr)));
    let narrow = run_in(&root, &["--color", "never", "--no-fold"], &[("COLUMNS", "40")]);
    assert_snapshot("no-fold-40.txt", &stable(&text(&narrow.stderr)));
    let ansi = run_in(&root, &["--color", "always"], &[("COLUMNS", "100")]);
    assert_snapshot("ansi-100.txt", &stable(&text(&ansi.stderr)));
    assert_snapshot("short.txt", &text(&run_in(&root, &["-o", "short"], &[]).stdout));
    assert_snapshot("github.txt", &text(&run_in(&root, &["-o", "github"], &[]).stdout));
}

/// `(path, row, column)` of every issue in upstream's JSON. A small scan rather than a parser,
/// because the format is fixed.
fn json_locations(json: &str, root: &str) -> Vec<(String, String, String)> {
    let field = |from: &str, key: &str| -> String {
        let start = from.find(key).unwrap() + key.len();
        let end = from[start..].find([',', '"']).unwrap();
        from[start..start + end].to_string()
    };
    json.split("{\"filePath\":\"")
        .skip(1)
        .map(|issue| {
            let path = &issue[..issue.find('"').unwrap()];
            // Upstream's JSON has native separators, escaped; the other formats always use `/`.
            let path = path.replace("\\\\", "/");
            let path = path.strip_prefix(root).unwrap_or(&path).trim_start_matches('/').to_string();
            (path, field(issue, "\"startRow\":"), field(issue, "\"startColumn\":"))
        })
        .collect()
}

/// Folding and formats change how an offense is shown, never whether it is.
fn assert_every_location_is_reported(root: &Path) {
    let json = text(&run_in(root, &["-o", "json"], &[]).stdout);
    // The child's working directory: resolved on Unix, and as it was given on Windows, where
    // canonicalizing would also expand 8.3 names such as the runner's `RUNNER~1`.
    let root_text = if cfg!(windows) { std::path::absolute(root).unwrap() } else { root.canonicalize().unwrap() };
    let locations = json_locations(&json, &lq_json::uri::from_native(&root_text.to_string_lossy()));
    let report = text(&run_in(root, &["--color", "never"], &[]).stderr);
    let short = text(&run_in(root, &["-o", "short"], &[]).stdout);
    let github = text(&run_in(root, &["-o", "github"], &[]).stdout);
    let rich = text(&run_in(root, &["-o", "json-rich"], &[]).stdout);
    for (path, row, column) in &locations {
        let location = format!("{path}:{row}:{column}");
        assert!(report.contains(&location), "{location} isn't in the text report");
        assert!(short.contains(&format!("{location}:")), "{location} isn't in short");
        let annotation = format!("file={path},line={row},col={column},");
        assert!(github.contains(&annotation), "{location} isn't in github");
    }
    assert_eq!(short.lines().count(), locations.len());
    assert_eq!(github.lines().count(), locations.len());
    assert_eq!(rich.matches("\"foldGroup\":").count(), locations.len());
}

fn reference_themes() -> Vec<PathBuf> {
    let reference = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference");
    let mut themes = Vec::new();
    for dir in ["corpus", "heldout"] {
        let Ok(entries) = std::fs::read_dir(reference.join(dir)) else { continue };
        themes.extend(entries.flatten().map(|e| e.path()).filter(|p| p.join("layout").is_dir()));
    }
    themes
}

#[test]
fn no_offense_goes_missing() {
    assert_every_location_is_reported(&fixture());
    for theme in reference_themes() {
        assert_every_location_is_reported(&theme);
    }
}

#[test]
fn renders_real_themes_at_any_width() {
    for theme in reference_themes().into_iter().chain([fixture()]) {
        for columns in ["40", "80", "200"] {
            for fold in [&["--color", "always"][..], &["--color", "always", "--no-fold"][..]] {
                let output = run_in(&theme, fold, &[("COLUMNS", columns)]);
                let stderr = text(&output.stderr);
                assert!(!stderr.contains("panicked"), "{}: {stderr}", theme.display());
                assert!(output.status.code().is_some_and(|c| c < 2), "{}: {stderr}", theme.display());
            }
        }
    }
}

#[test]
fn the_exit_code_never_depends_on_the_format() {
    let root = fixture();
    for level in ["crash", "error", "suggestion", "style", "warning", "info"] {
        let expected = run_in(&root, &["-o", "json", "--fail-level", level], &[]).status.code();
        for format in ["text", "short", "github", "json-rich", "upstream"] {
            let code = run_in(&root, &["-o", format, "--fail-level", level], &[]).status.code();
            assert_eq!(code, expected, "{format} at {level}");
        }
    }
}

#[test]
fn a_theme_file_cannot_drive_the_terminal() {
    let root = fixture();
    let plain = text(&run_in(&root, &["--color", "never"], &[]).stderr);
    assert!(!plain.contains('\u{1b}'), "an escape reached the report");
    assert!(plain.contains("esc\u{FFFD}[31mape.key"));
    let colored = text(&run_in(&root, &["--color", "always"], &[]).stderr);
    assert!(!colored.contains("\u{1b}[31mape"), "the file's escape reached the terminal");
    let short = text(&run_in(&root, &["-o", "short"], &[]).stdout);
    assert!(!short.contains('\u{1b}'));
    let github = text(&run_in(&root, &["-o", "github"], &[]).stdout);
    assert!(!github.contains('\u{1b}'));
}

#[test]
fn color_follows_the_conventions() {
    let root = fixture();
    let has_ansi = |env: &[(&str, &str)], args: &[&str]| text(&run_in(&root, args, env).stderr).contains('\u{1b}');
    assert!(!has_ansi(&[], &[]), "stderr is a pipe here");
    assert!(has_ansi(&[("FORCE_COLOR", "1")], &[]));
    assert!(has_ansi(&[("CLICOLOR_FORCE", "1")], &[]));
    assert!(!has_ansi(&[("NO_COLOR", "1"), ("FORCE_COLOR", "1")], &[]));
    assert!(has_ansi(&[("NO_COLOR", "1")], &["--color", "always"]));
}

/// `--auto-correct` skips a locale with a comment header, so the report must not promise it.
#[test]
fn only_promises_fixes_that_apply() {
    let root = fixture();
    let report = text(&run_in(&root, &["--color", "never"], &[("COLUMNS", "200")]).stderr);
    assert!(report.contains("--auto-correct can't change this file"), "{report}");
    assert!(report.contains("1 offense can be fixed with lq check --auto-correct"), "{report}");
    assert!(report.contains("a fix --auto-correct can't apply"), "{report}");
}

/// A small theme of its own, for cases that would only add noise to the snapshots.
fn small_theme(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&root);
    for (path, text) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    root
}

#[test]
fn an_error_on_the_first_byte_keeps_its_snippet() {
    let root = small_theme("first-byte", &[("locales/en.default.json", "x{\"a\":1}\n\n\n")]);
    let report = text(&run_in(&root, &["--color", "never"], &[]).stderr);
    assert!(report.contains("x{\"a\":1}"), "{report}");
}

#[test]
fn only_liquid_files_are_suggested() {
    let root = small_theme(
        "only-liquid",
        &[
            ("sections/a.liquid", "{% render 'card' %}\n"),
            ("snippets/cards/x.liquid", ""),
            ("snippets/card.bak", ""),
            ("snippets/other.liquid", "{% render 'a' %}\n"),
        ],
    );
    let report = text(&run_in(&root, &["--color", "never"], &[]).stderr);
    assert!(!report.contains("did you mean"), "{report}");
    assert!(report.contains("ignoreMissing"), "{report}");
}

#[test]
fn hints_only_reach_the_formats_that_show_them() {
    let root = fixture();
    let rich = text(&run_in(&root, &["-o", "json-rich"], &[]).stdout);
    assert!(rich.contains("did you mean 'card'?"), "{rich}");
    assert!(rich.contains("\"fixBlockedBy\":\"JSON.parse"), "{rich}");
    assert!(rich.contains("\"fixBlockedBy\":null"), "{rich}");
}

#[test]
fn hints_are_offered() {
    let root = fixture();
    let report = text(&run_in(&root, &["--color", "never"], &[("COLUMNS", "120")]).stderr);
    for hint in [
        "did you mean 'card'?",
        "did you mean 'subtitle'?",
        "did you mean 'upcase'?",
        "did you mean 'greeting.hello'?",
        "did you mean 'product'?",
        "ignoreMissing",
        "snippets/card.liquid declares",
    ] {
        assert!(report.contains(hint), "no {hint:?} in:\n{report}");
    }
}

#[test]
fn upstream_bugs_selects_upstreams_layout() {
    let root = fixture();
    let cards = text(&run_in(&root, &["--upstream-bugs"], &[]).stderr);
    assert!(cards.contains("Theme Check Summary."));
    assert!(text(&run_in(&root, &["-o", "upstream"], &[]).stderr).contains("Theme Check Summary."));
    assert!(!text(&run_in(&root, &["--upstream-bugs", "-o", "text"], &[]).stderr).contains("Theme Check Summary."));
    assert!(!text(&run_in(&root, &[], &[]).stderr).contains("Theme Check Summary."));
}

#[test]
fn the_upstream_json_carries_nothing_of_ours() {
    let json = text(&run_in(&fixture(), &["-o", "json"], &[]).stdout);
    for key in ["\"notes\"", "\"label\"", "\"foldGroup\"", "\"schemaVersion\""] {
        assert!(!json.contains(key), "{key} in the upstream JSON");
    }
}

#[test]
fn explain_describes_a_check() {
    let run = |name: &str| Command::new(LQ).args(["explain", name]).output().unwrap();
    let out = run("missingtemplate");
    assert_eq!(out.status.code(), Some(0));
    let stdout = text(&out.stdout);
    assert!(stdout.starts_with("MissingTemplate\n"));
    assert!(stdout.contains("ignoreMissing"));
    assert!(stdout.contains("theme-check-disable MissingTemplate"));
    assert!(stdout.contains("https://shopify.dev/"));
    let out = run("MissingTemplat");
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("did you mean 'MissingTemplate'?"));
}
