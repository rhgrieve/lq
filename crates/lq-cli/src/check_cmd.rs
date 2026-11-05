//! `lq check`: the offline flags of `shopify theme check`, and debug entry points.

use lq_check::deps::NodeDependencies;
use lq_check::fixes::fix_all;
use lq_check::registry::{CONFIG_REGISTRY, REGISTRY};
use lq_check::{Offense, Severity};
use lq_json::js::quote;
use lq_json::{uri, Object, Value};
use lq_theme::config::{load_config, Config, ConfigError};
use lq_theme::discovery::get_theme_files;
use lq_theme::divergence::{parse_divergence_flag, Divergences};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

/// Upstream's parity target: what `--version` reports.
const THEME_CHECK_VERSION: &str = "3.31.0";

/// Worker threads parse and walk deep trees too.
const WORKER_STACK: usize = 1 << 30;

#[derive(Default)]
struct Flags {
    path: Option<String>,
    config: Option<String>,
    output: Option<String>,
    json: bool,
    fail_level: Option<String>,
    auto_correct: bool,
    init: bool,
    list: bool,
    print: bool,
    version: bool,
    help: bool,
    timings: bool,
    divergences: Divergences,
    /// Files or directories to report offenses for. The whole theme is still loaded.
    files: Vec<String>,
}

pub const CHECK_HELP: &str = "\
Check a theme, like `shopify theme check` (theme-check 3.31.0). Never touches the network.

Usage: lq check [options] [files or directories...]

With no file arguments, reports the whole theme. With them, the whole theme is still loaded,
so cross-file checks stay correct, but only offenses in those files are reported and decide
the exit code. Paths are relative to the current directory or to the theme root; a path that
doesn't exist is skipped with a warning. Fits lefthook's {staged_files}.

Options:
  -p, --path <dir>         Theme root [default: current directory]
  -C, --config <file>      Config file, or theme-check:all, :recommended, :theme-app-extension,
                           :nothing [default: .theme-check.yml in the root]
  -o, --output <format>    text or json [default: text]
  -j, --json               Same as --output json
      --fail-level <level> crash, error, suggestion, style, warning or info [default: error]
  -a, --auto-correct       Apply safe fixes in place
      --upstream-bugs      Reproduce upstream exactly, bugs included: see DIVERGENCES.md
      --timings            Print the time spent in each phase to stderr
      --print              Print the resolved config and exit
      --list               List the enabled checks and exit
      --init               Write a .theme-check.yml to the theme root and exit
  -v, --version            Print the theme-check version this targets and exit
  -h, --help               Print this help

Exit code: 0 clean, 1 offenses at or above the fail level or a bad config, 2 bad usage.
Text output goes to stderr and JSON to stdout, as upstream does.
";

const LEVELS: [&str; 6] = ["crash", "error", "suggestion", "style", "warning", "info"];

fn parse_flags(args: &[String]) -> Result<Flags, String> {
    let mut f = Flags::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if arg == "--" {
            f.files.extend(args[i..].iter().cloned());
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            f.files.push(arg.clone());
            continue;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with('-') => (n, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let value = |i: &mut usize| -> Result<String, String> {
            if let Some(v) = inline.clone() {
                return Ok(v);
            }
            let v = args.get(*i).cloned().ok_or_else(|| format!("Flag {name} expects a value"))?;
            *i += 1;
            Ok(v)
        };
        match name {
            "--path" | "-p" => f.path = Some(value(&mut i)?),
            "--config" | "-C" => f.config = Some(value(&mut i)?),
            "--output" | "-o" => {
                let v = value(&mut i)?;
                if v != "text" && v != "json" {
                    return Err(format!("Expected --output={v} to be one of: text, json"));
                }
                f.output = Some(v);
            }
            "--fail-level" => {
                let v = value(&mut i)?;
                if !LEVELS.contains(&v.as_str()) {
                    return Err(format!("Expected --fail-level={v} to be one of: {}", LEVELS.join(", ")));
                }
                f.fail_level = Some(v);
            }
            "--json" | "-j" => f.json = true,
            "--auto-correct" | "-a" => f.auto_correct = true,
            "--init" => f.init = true,
            "--list" => f.list = true,
            "--print" => f.print = true,
            "--version" | "-v" => f.version = true,
            "--help" | "-h" => f.help = true,
            "--timings" => f.timings = true,
            "--upstream-bugs" => f.divergences = Divergences::NONE,
            "--only-divergence" => {
                let v = value(&mut i)?;
                match parse_divergence_flag(name, Some(&v)) {
                    Some(Ok(d)) => f.divergences = d,
                    Some(Err(e)) => return Err(e),
                    None => {}
                }
            }
            other => return Err(format!("Nonexistent flag: {other}")),
        }
    }
    Ok(f)
}

/// `Ae(failLevel)`: the severity at or below which the run fails.
fn fail_threshold(level: &str) -> Option<Severity> {
    match level {
        "error" => Some(Severity::Error),
        "suggestion" | "warning" => Some(Severity::Warning),
        "style" | "info" => Some(Severity::Info),
        _ => None,
    }
}

fn absolute(p: &str) -> String {
    std::path::absolute(p).map_or_else(|_| p.to_string(), |a| a.to_string_lossy().into_owned())
}

const LEGACY: [(&str, &str); 8] = [
    ("default", "theme-check:recommended"),
    ("recommended", "theme-check:recommended"),
    ("all", "theme-check:all"),
    ("nothing", "theme-check:nothing"),
    ("theme_app_extension", "theme-check:theme-app-extension"),
    ("theme_app_extensions", "theme-check:theme-app-extension"),
    ("theme-app-extension", "theme-check:theme-app-extension"),
    ("theme-app-extensions", "theme-check:theme-app-extension"),
];

const MODERN: [&str; 4] =
    ["theme-check:nothing", "theme-check:recommended", "theme-check:theme-app-extension", "theme-check:all"];

/// The `--config` value after the bundle's legacy mapping; paths made absolute.
fn resolve_config_flag(config: Option<&str>) -> Option<String> {
    let config = config?;
    if let Some(name) = config.strip_prefix(':') {
        if let Some((_, modern)) = LEGACY.iter().find(|(k, _)| *k == name) {
            return Some((*modern).to_string());
        }
    }
    if MODERN.contains(&config) {
        return Some(config.to_string());
    }
    Some(absolute(config))
}

/// `G(configPath, fn)`: the bundle's friendlier config errors.
fn load(config: Option<&str>, root: &str) -> Result<Config, CliError> {
    if let Some(path) = config.filter(|c| !MODERN.contains(c)) {
        if Path::new(path).is_dir() {
            return Err(CliError {
                message: format!("Theme Check config must be a file, but {path} is a directory."),
                next_steps: Some("Pass the path to a .theme-check.yml file to --config, or omit --config to use the .theme-check.yml in your theme root. To choose the theme directory, use --path."),
            });
        }
    }
    load_config(config, root, CONFIG_REGISTRY).map_err(|e: ConfigError| {
        match config.filter(|c| !MODERN.contains(c) && !Path::new(c).exists()) {
            Some(path) => CliError { message: format!("Theme Check config file not found: {path}"), next_steps: None },
            None => CliError { message: e.message, next_steps: None },
        }
    })
}

struct CliError {
    message: String,
    next_steps: Option<&'static str>,
}

// ---- text rendering -----------------------------------------------------------

/// The width of a char in a terminal cell, as `string-width` counts it.
fn cell_width(c: char) -> usize {
    let u = c as u32;
    if c.is_control() || (0x300..=0x36F).contains(&u) || (0x200B..=0x200F).contains(&u) || (0xFE00..=0xFE0F).contains(&u) {
        return 0;
    }
    let wide = (0x1100..=0x115F).contains(&u)
        || (0x2E80..=0x303E).contains(&u)
        || (0x3041..=0x33FF).contains(&u)
        || (0x3400..=0x4DBF).contains(&u)
        || (0x4E00..=0x9FFF).contains(&u)
        || (0xA000..=0xA4CF).contains(&u)
        || (0xAC00..=0xD7A3).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFE30..=0xFE6F).contains(&u)
        || (0xFF00..=0xFF60).contains(&u)
        || (0xFFE0..=0xFFE6).contains(&u)
        || (0x1F300..=0x1F64F).contains(&u)
        || (0x1F900..=0x1F9FF).contains(&u)
        || (0x20000..=0x3FFFD).contains(&u);
    if wide {
        2
    } else {
        1
    }
}

fn width(s: &str) -> usize {
    s.chars().map(cell_width).sum()
}

/// `wrapAnsi(line, columns, { trim: false, hard: true })` for one line of plain text.
fn wrap_line(line: &str, columns: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    let words: Vec<&str> = line.split(' ').collect();
    let mut rows = vec![String::new()];
    let hard_wrap = |rows: &mut Vec<String>, word: &str| {
        let mut visible = width(rows.last().unwrap());
        for c in word.chars() {
            let w = cell_width(c);
            if visible + w > columns && visible > 0 {
                rows.push(String::new());
                visible = 0;
            }
            rows.last_mut().unwrap().push(c);
            visible += w;
            if visible == columns {
                rows.push(String::new());
                visible = 0;
            }
        }
        if rows.len() > 1 && rows.last().unwrap().is_empty() {
            rows.pop();
        }
    };
    for (index, word) in words.iter().enumerate() {
        let mut row_length = width(rows.last().unwrap());
        if index != 0 {
            if row_length >= columns {
                rows.push(String::new());
                row_length = 0;
            }
            if row_length > 0 {
                rows.last_mut().unwrap().push(' ');
                row_length += 1;
            }
        }
        let len = width(word);
        if len > columns {
            let remaining = columns - row_length;
            let this_line = 1 + (len - remaining - 1) / columns;
            let next_line = (len - 1) / columns;
            if next_line < this_line {
                rows.push(String::new());
            }
            hard_wrap(&mut rows, word);
            continue;
        }
        if row_length + len > columns && row_length > 0 {
            rows.push(String::new());
        }
        rows.last_mut().unwrap().push_str(word);
    }
    rows
}

/// An Ink `Alert` card, as the CLI draws it without color at width 80.
fn card(kind: &str, sections: &[String]) -> String {
    const W: usize = 80;
    let mut out = String::new();
    out.push_str("╭─ ");
    out.push_str(kind);
    out.push(' ');
    for _ in 0..W - 5 - kind.len() {
        out.push('─');
    }
    out.push_str("╮\n");
    let row = |out: &mut String, text: &str| {
        out.push_str("│  ");
        out.push_str(text);
        for _ in width(text)..W - 6 {
            out.push(' ');
        }
        out.push_str("  │\n");
    };
    row(&mut out, "");
    for (i, section) in sections.iter().enumerate() {
        if i > 0 {
            row(&mut out, "");
        }
        for line in section.split('\n') {
            for wrapped in wrap_line(line, W - 6) {
                row(&mut out, &wrapped);
            }
        }
    }
    row(&mut out, "");
    out.push('╰');
    for _ in 0..W - 2 {
        out.push('─');
    }
    out.push_str("╯\n");
    out
}

fn tag(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "[error]:",
        Severity::Warning => "[warning]:",
        Severity::Info => "[info]:",
    }
}

/// `Ne(source, startLine, endLine)`.
fn code_lines(source: &str, start: u32, end: u32) -> String {
    let lines: Vec<&str> = source.split('\n').collect();
    let (s, e) = (start as usize, (end as usize + 1).min(lines.len()));
    let slice = lines.get(s..e).unwrap_or(&[]);
    let single = slice.len() == 1;
    slice
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let text = if single { lq_syntax::js::trim(l) } else { l };
            format!("{}  {text}", s + i + 1)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn grouped(offenses: &[Offense]) -> Vec<(String, Vec<&Offense>)> {
    // `H`: by fsPath in first-offense order, each sorted (stably) by severity.
    let mut groups: Vec<(String, Vec<&Offense>)> = Vec::new();
    let mut index = BTreeMap::new();
    for o in offenses {
        let path = uri::fs_path(&o.uri);
        let i = *index.entry(path.clone()).or_insert_with(|| {
            groups.push((path, Vec::new()));
            groups.len() - 1
        });
        groups[i].1.push(o);
    }
    for (_, list) in &mut groups {
        list.sort_by_key(|o| o.severity);
    }
    groups
}

fn counts(list: &[&Offense]) -> (usize, usize, usize) {
    let n = |s| list.iter().filter(|o| o.severity == s).count();
    (n(Severity::Error), n(Severity::Warning), n(Severity::Info))
}

fn render_text(root: &str, groups: &[(String, Vec<&Offense>)], sources: &[(String, String)], inspected: usize, total: usize) -> String {
    let source_of: BTreeMap<&str, &str> = sources.iter().map(|(u, s)| (u.as_str(), s.as_str())).collect();
    let mut out = String::from("\n");
    let mut sorted: Vec<&(String, Vec<&Offense>)> = groups.iter().collect();
    sorted.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
    for (path, list) in sorted {
        // `c.replace(path, '').slice(1)`.
        let headline = match path.find(root) {
            Some(i) => format!("{}{}", &path[..i], &path[i + root.len()..]),
            None => path.clone(),
        };
        let headline: String = headline.chars().skip(1).collect();
        let mut body = String::new();
        for (i, o) in list.iter().enumerate() {
            if i > 0 {
                body.push_str("\n\n");
            }
            body.push('\n');
            body.push_str(tag(o.severity));
            body.push(' ');
            body.push_str(o.check);
            body.push('\n');
            body.push_str(&o.message);
            if let Some(source) = source_of.get(o.uri.as_str()) {
                body.push_str("\n\n");
                body.push_str(&code_lines(source, o.start.line, o.end.line));
            }
        }
        out.push_str(&card("info", &[headline, body]));
        out.push('\n');
    }
    let mut lines = vec![format!("{inspected} files inspected")];
    if total == 0 {
        lines[0].push_str(" with no offenses found.");
    } else {
        lines[0].push_str(&format!(" with {total} total offenses found across {} files.", groups.len()));
        let all: Vec<&Offense> = groups.iter().flat_map(|(_, l)| l.iter().copied()).collect();
        let (e, w, i) = counts(&all);
        if e > 0 {
            lines.push(format!("{e} errors."));
        }
        if w > 0 {
            lines.push(format!("{w} warnings."));
        }
        if i > 0 {
            lines.push(format!("{i} info issues."));
        }
    }
    let kind = if total > 0 { "info" } else { "success" };
    out.push_str(&card(kind, &["Theme Check Summary.".to_string(), lines.join("\n")]));
    out.push('\n');
    out
}

/// `J(result)`: the `ThemeCheckResult` JSON. Rows and columns are one-based.
fn render_json(valid: bool, groups: &[(String, Vec<&Offense>)]) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("{\"valid\":");
    out.push_str(if valid { "true" } else { "false" });
    out.push_str(",\"issues\":[");
    let mut first = true;
    let (mut errors, mut warnings, mut infos) = (0, 0, 0);
    for (path, list) in groups {
        let (e, w, i) = counts(list);
        errors += e;
        warnings += w;
        infos += i;
        let file_path = quote(path);
        for o in list {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&format!(
                "{{\"filePath\":{file_path},\"check\":{},\"severity\":\"{}\",\"startRow\":{},\"startColumn\":{},\"endRow\":{},\"endColumn\":{},\"message\":{}}}",
                quote(o.check),
                o.severity.as_str(),
                o.start.line + 1,
                o.start.character + 1,
                o.end.line + 1,
                o.end.character + 1,
                quote(&o.message),
            ));
        }
    }
    out.push_str(&format!("],\"errorCount\":{errors},\"warningCount\":{warnings},\"infoCount\":{infos}}}"));
    out
}

// ---- --print, --list, --init ---------------------------------------------------

fn string_array(items: &[String]) -> Value {
    Value::Array(items.iter().map(|s| Value::String(s.clone())).collect())
}

fn print_config(config: &Config) {
    let mut o = Object::new();
    o.insert("extends", Value::Array(Vec::new()));
    o.insert("ignore", string_array(config.ignore.as_deref().unwrap_or(&[])));
    o.insert("rootUri", Value::String(config.root_uri.clone()));
    for (k, v) in config.settings.iter() {
        o.insert(k, v.clone());
    }
    println!("{}", lq_theme::yaml::stringify(&Value::Object(o)));
}

fn list_checks(config: &Config) {
    let ignore = config.ignore.clone().unwrap_or_default();
    let ignored_patterns = format!("[{}]", ignore.join(", "));
    let mut out = Object::new();
    for (code, settings) in config.settings.iter() {
        let Some(settings) = settings.as_object() else { continue };
        if !settings.get("enabled").is_some_and(Value::is_truthy) {
            continue;
        }
        let severity = settings
            .get("severity")
            .and_then(Value::as_f64)
            .and_then(Severity::from_number)
            .unwrap_or(Severity::Info);
        let mut entry = Object::new();
        entry.insert("severity", Value::String(severity.as_str().to_string()));
        if config.checks.contains(&code) {
            if let Some(def) = REGISTRY.iter().find(|d| d.meta.code == code) {
                entry.insert("description", Value::String(def.meta.description.to_string()));
                if let Some(url) = def.meta.url {
                    entry.insert("doc", Value::String(url.to_string()));
                }
            }
        }
        entry.insert("ignored_patterns", Value::String(ignored_patterns.clone()));
        for (k, v) in settings.iter() {
            if k != "severity" && k != "enabled" {
                entry.insert(k, v.clone());
            }
        }
        out.insert(code, Value::Object(entry));
    }
    println!("{}", lq_theme::yaml::stringify(&Value::Object(out)));
}

fn init(root: &str) -> Result<i32, CliError> {
    let target = Path::new(root).join(".theme-check.yml");
    if target.exists() {
        eprintln!("\n.theme-check.yml already exists at {root}");
        return Ok(0);
    }
    let config = load(None, root)?;
    let commented: Vec<String> = lq_theme::yaml::stringify(&Value::Object(config.settings.clone()))
        .split('\n')
        .map(|l| format!("# {l}"))
        .collect();
    let mut head = Object::new();
    head.insert("extends", Value::String("theme-check:recommended".into()));
    head.insert("ignore", Value::Array(vec![Value::String("node_modules/**".into())]));
    let text = format!("{}{}", lq_theme::yaml::stringify(&Value::Object(head)), commented.join("\n"));
    std::fs::write(&target, text).map_err(|e| CliError { message: e.to_string(), next_steps: None })?;
    eprintln!("\n\u{2705} Success! Created .theme-check.yml at {root}.");
    Ok(0)
}

// ---- the command ---------------------------------------------------------------

/// `lq check`. Exits the process itself, so ASTs are never dropped.
pub fn check(args: &[String]) -> i32 {
    let flags = match parse_flags(args) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if flags.help {
        print!("{CHECK_HELP}");
        return 0;
    }
    let json = flags.json || flags.output.as_deref() == Some("json");
    if json {
        let conflict = [(flags.init, "init"), (flags.version, "version"), (flags.print, "print"), (flags.list, "list")]
            .into_iter()
            .find(|(on, _)| *on);
        if let Some((_, name)) = conflict {
            eprintln!("JSON output can't be used with --{name}.");
            return 2;
        }
    }
    let root = absolute(flags.path.as_deref().unwrap_or("."));
    let root = root.strip_suffix('/').filter(|r| !r.is_empty()).map_or(root.clone(), String::from);
    let root = uri::posix_normalize(&root);
    let config_path = resolve_config_flag(flags.config.as_deref());

    let result = (|| -> Result<i32, CliError> {
        if flags.init {
            return init(&root);
        }
        if flags.version {
            println!("{THEME_CHECK_VERSION}");
            return Ok(0);
        }
        if flags.print {
            print_config(&load(config_path.as_deref(), &root)?);
            return Ok(0);
        }
        if flags.list {
            list_checks(&load(config_path.as_deref(), &root)?);
            return Ok(0);
        }
        run_check(&flags, json, &root, config_path.as_deref())
    })();
    match result {
        Ok(code) => code,
        Err(e) => {
            if json {
                println!("{{\"error\":{{\"type\":\"abort\",\"message\":{}}}}}", quote(&e.message));
            } else {
                let mut sections = vec![e.message.clone()];
                if let Some(next) = e.next_steps {
                    sections.push(next.to_string());
                }
                eprint!("\n{}\n", card("error", &sections));
            }
            1
        }
    }
}

fn run_check(flags: &Flags, json: bool, root: &str, config_path: Option<&str>) -> Result<i32, CliError> {
    let timing = flags.timings || std::env::var_os("LQ_TIMING").is_some();
    let t0 = Instant::now();
    let mut config = load(config_path, root)?;
    config.divergences = flags.divergences;
    if let Some(e) = config.ignore_type_error() {
        return Err(CliError { message: e.message, next_steps: None });
    }
    let t_config = t0.elapsed();

    let pool = rayon::ThreadPoolBuilder::new()
        .stack_size(WORKER_STACK)
        .build()
        .map_err(|e| CliError { message: e.to_string(), next_steps: None })?;

    let t = Instant::now();
    let paths = get_theme_files(&config);
    let t_discovery = t.elapsed();

    pool.install(|| {
        let t = Instant::now();
        let sources: Vec<(String, String)> = paths
            .par_iter()
            .filter_map(|p| {
                let source = lq_check::fs::read_utf8_lossy(Path::new(p)).ok()?;
                Some((uri::file_uri(p), source))
            })
            .collect();
        let t_read = t.elapsed();

        let t = Instant::now();
        let theme = lq_check::parse_theme(&sources);
        let t_parse = t.elapsed();

        let t = Instant::now();
        let deps = NodeDependencies::new(&theme, &config, root);
        let mut output = lq_check::check(&theme, &config, &deps);
        let t_checks = t.elapsed();
        let selection = Selection::new(&flags.files, root);
        output.offenses.retain(|o| selection.contains(&uri::fs_path(&o.uri)));
        let inspected = sources.iter().filter(|(u, _)| selection.contains(&uri::fs_path(u))).count();
        if let Some(reason) = deps.graph_failure().filter(|_| config.divergences.contains(lq_check::Divergence::GraphFailure)) {
            eprintln!("[lq] warning: the theme graph failed to build, so OrphanedSnippet and ValidScopedCSSClass were skipped: {reason}");
        }

        let t = Instant::now();
        let threshold = flags.fail_level.as_deref().map_or(Some(Severity::Error), fail_threshold);
        let valid = !threshold.is_some_and(|r| output.offenses.iter().any(|o| o.severity <= r));
        let groups = grouped(&output.offenses);
        if !json {
            eprint!("{}", render_text(root, &groups, &sources, inspected, output.offenses.len()));
        }
        if flags.auto_correct {
            let keep_going = config.divergences.contains(lq_check::Divergence::AutofixAbort);
            apply_autofix(&sources, &output.offenses, keep_going).map_err(|message| CliError { message, next_steps: None })?;
        }
        if json {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            let _ = writeln!(lock, "{}", render_json(valid, &groups));
            let _ = lock.flush();
        }
        let t_output = t.elapsed();

        let missing: Vec<&str> = config
            .checks
            .iter()
            .copied()
            .filter(|c| REGISTRY.iter().any(|d| d.meta.code == *c && !d.imp.is_implemented()))
            .collect();
        if !missing.is_empty() {
            eprintln!(
                "[lq] note: {} enabled checks are not implemented yet and did not run",
                missing.len()
            );
        }
        for (code, u, e) in &output.errors {
            if std::env::var_os("LQ_DEBUG").is_some() {
                eprintln!("[lq] {code} failed on {u}: {}: {}", e.name, e.message);
            }
        }
        if timing {
            eprintln!(
                "[lq] timing: config {t_config:.2?}, discovery {t_discovery:.2?} ({} files), read {t_read:.2?}, parse {t_parse:.2?}, checks {t_checks:.2?}, output {t_output:.2?}, total {:.2?}",
                sources.len(),
                t0.elapsed()
            );
        }
        // Leave without dropping ASTs: exiting is the fast way to free them.
        let code = i32::from(!valid);
        std::process::exit(code);
    })
}

/// The files named on the command line. Empty means everything.
struct Selection {
    files: Vec<String>,
    dirs: Vec<String>,
    all: bool,
}

impl Selection {
    fn new(args: &[String], root: &str) -> Selection {
        let mut s = Selection { files: Vec::new(), dirs: Vec::new(), all: args.is_empty() };
        for arg in args {
            let from_cwd = uri::posix_normalize(&absolute(arg));
            let from_root = uri::posix_normalize(&Path::new(root).join(arg).to_string_lossy());
            let Some(path) = [from_cwd, from_root].into_iter().find(|p| Path::new(p).exists()) else {
                eprintln!("[lq] warning: {arg} doesn't exist, skipping it");
                continue;
            };
            if Path::new(&path).is_dir() {
                s.dirs.push(format!("{}/", path.trim_end_matches('/')));
            } else {
                s.files.push(path);
            }
        }
        s
    }

    fn contains(&self, path: &str) -> bool {
        self.all || self.files.iter().any(|f| f == path) || self.dirs.iter().any(|d| path.starts_with(d))
    }
}

/// `autofix(theme, offenses)`. Upstream stops at the first file whose fix can't be applied;
/// with `keep_going` that file is skipped with a warning and the rest are still fixed.
fn apply_autofix(sources: &[(String, String)], offenses: &[Offense], keep_going: bool) -> Result<(), String> {
    for (u, source) in sources {
        let fixes: Vec<_> = offenses.iter().filter(|o| o.uri == *u).filter_map(|o| o.fix.as_ref()).collect();
        if fixes.is_empty() {
            continue;
        }
        let fixed = match fix_all(source, &fixes) {
            Ok(fixed) => fixed,
            Err(message) if keep_going => {
                eprintln!("[lq] warning: skipped --auto-correct for {}: {message}", uri::fs_path(u));
                continue;
            }
            Err(message) => return Err(message),
        };
        std::fs::write(uri::fs_path(u), fixed).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---- debug ----------------------------------------------------------------------

/// `--upstream-bugs` or `--only-divergence <ID>` of a debug command.
fn debug_divergences(args: &[String]) -> Result<Divergences, String> {
    let mut out = Divergences::ALL;
    for (i, flag) in args.iter().enumerate() {
        if let Some(parsed) = parse_divergence_flag(flag, args.get(i + 1).map(String::as_str)) {
            out = parsed?;
        }
    }
    Ok(out)
}

/// `lq debug-offenses --path <root>`: JSON of every offense with what its fix and suggestions
/// produce, which `lq check` doesn't print. `scripts/parity/divergences.mjs` uses it.
pub fn debug_offenses(args: &[String]) -> i32 {
    let Some(root) = args.iter().position(|a| a == "--path").and_then(|i| args.get(i + 1)) else {
        eprintln!("usage: lq debug-offenses --path <root> [--upstream-bugs] [--only-divergence <ID>]");
        return 2;
    };
    let divergences = match debug_divergences(args) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let root = uri::posix_normalize(&absolute(root));
    let Ok(mut config) = load(None, &root) else { return 1 };
    config.divergences = divergences;
    let Ok(pool) = rayon::ThreadPoolBuilder::new().stack_size(WORKER_STACK).build() else { return 1 };
    pool.install(|| {
        let sources: Vec<(String, String)> = get_theme_files(&config)
            .iter()
            .filter_map(|p| Some((uri::file_uri(p), lq_check::fs::read_utf8_lossy(Path::new(p)).ok()?)))
            .collect();
        let theme = lq_check::parse_theme(&sources);
        let deps = NodeDependencies::new(&theme, &config, &root);
        let output = lq_check::check(&theme, &config, &deps);
        let source_of: BTreeMap<&str, &str> = sources.iter().map(|(u, s)| (u.as_str(), s.as_str())).collect();
        let applied = |u: &str, fix: &lq_check::fixes::Fix| -> Value {
            source_of.get(u).and_then(|s| fix_all(s, &[fix]).ok()).map_or(Value::Null, Value::String)
        };
        let number = |n: u32| Value::Number(f64::from(n));
        let list: Vec<Value> = output
            .offenses
            .iter()
            .map(|o| {
                let mut e = Object::new();
                e.insert("file", Value::String(uri::relative(&o.uri, &config.root_uri)));
                e.insert("check", Value::String(o.check.to_string()));
                e.insert("message", Value::String(o.message.clone()));
                e.insert("start", Value::Array(vec![number(o.start.line), number(o.start.character)]));
                e.insert("end", Value::Array(vec![number(o.end.line), number(o.end.character)]));
                e.insert("fixed", o.fix.as_ref().map_or(Value::Null, |f| applied(&o.uri, f)));
                let suggest = o.suggest.as_deref().unwrap_or(&[]).iter().map(|s| {
                    let mut m = Object::new();
                    m.insert("message", Value::String(s.message.clone()));
                    m.insert("applied", applied(&o.uri, &s.fix));
                    Value::Object(m)
                });
                e.insert("suggest", Value::Array(suggest.collect()));
                Value::Object(e)
            })
            .collect();
        println!("{}", Value::Array(list).to_json());
        let _ = std::io::stdout().flush();
        std::process::exit(0)
    })
}

/// `lq debug-check-golden <file>`. The contract is in `scripts/parity/goldens.mjs`.
pub fn debug_check_golden(args: &[String]) -> i32 {
    let Some(file) = args.iter().find(|a| !a.starts_with("--") ) else {
        eprintln!("usage: lq debug-check-golden <file> [--show <n>]");
        return 2;
    };
    let show = args
        .iter()
        .position(|a| a == "--show")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(usize::MAX);
    let file = if args.iter().position(|a| a == "--show").is_some_and(|i| args.get(i + 1) == Some(file)) {
        match args.iter().skip(2).find(|a| !a.starts_with("--")) {
            Some(f) => f,
            None => return 2,
        }
    } else {
        file
    };
    let pool = match rayon::ThreadPoolBuilder::new().stack_size(WORKER_STACK).build() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let divergences = match debug_divergences(args) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let summary = match pool.install(|| lq_check::golden::replay_file(Path::new(file), divergences)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{file}: {e}");
            return 2;
        }
    };
    // A file with no runnable record because nothing is ported is "not implemented".
    let not_implemented = summary.not_implemented > 0;
    let failures: Vec<String> = summary
        .failures
        .iter()
        .take(show)
        .map(|(t, d)| format!("{{\"test\":{},\"diff\":{}}}", quote(t), quote(d)))
        .collect();
    println!(
        "{{\"check\":{},\"total\":{},\"passed\":{},\"failed\":{},\"unreplayable\":{},\"notImplemented\":{},\"skipped\":{},\"failures\":[{}]}}",
        quote(&summary.check),
        summary.total,
        summary.passed,
        summary.failed,
        summary.unreplayable,
        not_implemented,
        summary.not_implemented,
        failures.join(",")
    );
    i32::from(summary.failed > 0)
}

/// One line per file: the visit sequence, for `scripts/parity/visit.mjs`. `N` is the
/// number of ancestors.
pub fn debug_visit(files: &[String]) -> i32 {
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    for path in files {
        let Ok(source) = lq_check::fs::read_utf8_lossy(Path::new(path)) else { continue };
        let line = match lq_syntax::parse_strict(&source) {
            Err(e) => format!("error:{}", e.name),
            Ok(doc) => {
                let mut items: Vec<String> = Vec::new();
                lq_check::visitor::visit(&doc.root, &mut |node, ancestors| {
                    items.push(format!("{}:{}", node.node_type(), ancestors.len()));
                });
                items.join(" ")
            }
        };
        if writeln!(out, "{line}").is_err() {
            return 1;
        }
    }
    i32::from(out.flush().is_err())
}

/// `lq debug-checks`: the registry, one JSON object per line.
pub fn debug_checks() -> i32 {
    for def in REGISTRY {
        println!(
            "{{\"code\":{},\"kind\":\"{}\",\"implemented\":{}}}",
            quote(def.meta.code),
            def.meta.kind.as_str(),
            def.imp.is_implemented()
        );
    }
    0
}

/// `lq debug-engine --path <root>`: JSON of what `scripts/parity/engine.mjs` compares
/// against upstream: the theme graph, doc definitions and CSS classes.
pub fn debug_engine(args: &[String]) -> i32 {
    use lq_check::deps::Dependencies;
    let Some(root) = args.iter().position(|a| a == "--path").and_then(|i| args.get(i + 1)) else {
        eprintln!("usage: lq debug-engine --path <root>");
        return 2;
    };
    let root = absolute(root);
    let Ok(mut config) = load(None, &root) else { return 1 };
    match debug_divergences(args) {
        Ok(d) => config.divergences = d,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    }
    let sources: Vec<(String, String)> = get_theme_files(&config)
        .iter()
        .filter_map(|p| Some((uri::file_uri(p), lq_check::fs::read_utf8_lossy(Path::new(p)).ok()?)))
        .collect();
    let theme: lq_check::Theme<'_> = sources.iter().map(|(u, s)| lq_check::to_source_code(u, s)).collect();
    let deps = NodeDependencies::new(&theme, &config, &root);
    let graph = lq_check::graph::build_theme_graph(&config.root_uri, &deps, &theme, config.context, config.divergences);
    let mut out = Object::new();
    match graph {
        Err(e) => out.insert("graphError", Value::String(e)),
        Ok(g) => {
            let mut modules = Object::new();
            let refs = |list: &[lq_check::graph::Reference]| -> Value {
                let mut v: Vec<String> = list
                    .iter()
                    .map(|r| format!("{:?} {} -> {}", r.ty, r.source.uri, r.target.uri))
                    .collect();
                v.sort();
                Value::Array(v.into_iter().map(Value::String).collect())
            };
            for (u, m) in &g.modules {
                let mut o = Object::new();
                o.insert("exists", Value::Bool(m.exists == Some(true)));
                o.insert("dependencies", refs(&m.dependencies));
                o.insert("references", refs(&m.references));
                modules.insert(u.clone(), Value::Object(o));
            }
            out.insert("modules", Value::Object(modules));
        }
    }
    let mut docs = Object::new();
    let mut css = Object::new();
    let mut params = Object::new();
    let root_uri = uri::Uri::file(&root).to_string_with(false);
    let mut uris: Vec<String> = sources.iter().map(|(u, _)| u.clone()).filter(|u| u.ends_with(".liquid")).collect();
    if let Ok(files) = lq_check::fs::NodeFileSystem.read_dir_for_debug(&uri::join(&config.root_uri, &["assets"])) {
        uris.extend(files.into_iter().filter(|u| u.ends_with(".css")));
    }
    for u in &uris {
        let set = deps.css_classes_for_uri(u);
        let mut v: Vec<&String> = set.iter().collect();
        v.sort();
        css.insert(u.clone(), Value::Array(v.into_iter().map(|c| Value::String(c.clone())).collect()));
    }
    for (u, _) in &sources {
        let rel = uri::relative(u, &root_uri);
        let Some(d) = deps.doc_definition(&rel) else { continue };
        let content = d.liquid_doc.as_ref();
        let mut o = Object::new();
        o.insert("hasDoc", Value::Bool(content.is_some()));
        if let Some(c) = content {
            let ps: Vec<Value> = c.parameters.iter().flatten().map(|p| {
                let opt = |x: &Option<String>| x.clone().map_or(Value::Null, Value::String);
                Value::Array(vec![Value::String(p.name.clone()), opt(&p.ty), opt(&p.description), Value::Bool(p.required)])
            }).collect();
            o.insert("parameters", Value::Array(ps));
            o.insert("examples", Value::Array(c.examples.iter().flatten().map(|e| Value::String(e.clone())).collect()));
            o.insert("description", c.description.clone().map_or(Value::Null, Value::String));
        }
        docs.insert(rel.clone(), Value::Object(o));
        if u.contains("/blocks/") {
            let name = uri::basename(u, Some(".liquid"));
            let value = match deps.block_schema(&name).and_then(|_| lq_check::block_parameters::make_for_debug(&deps, &name)) {
                None => Value::Null,
                Some(list) => Value::Array(list.0.iter().map(|p| Value::Array(vec![
                    Value::String(p.name.clone()),
                    p.ty.clone().map_or(Value::Null, Value::String),
                    Value::Bool(p.required),
                ])).collect()),
            };
            params.insert(name, value);
        }
    }
    out.insert("docs", Value::Object(docs));
    out.insert("css", Value::Object(css));
    out.insert("blockParameters", Value::Object(params));
    println!("{}", Value::Object(out).to_json());
    let _ = std::io::stdout().flush();
    std::process::exit(0)
}
