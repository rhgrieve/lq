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
use crate::filter::Filter;
use crate::report::term::{style, ColorChoice, Term};
use crate::report::upstream::{card, grouped, render_json, render_text};
use crate::report::{fail_threshold, machine, rail, Report};
use std::collections::BTreeMap;
use std::io::Write;
use std::mem::ManuallyDrop;
use std::path::Path;
use std::time::Instant;

/// Upstream's parity target: what `--version` reports.
const THEME_CHECK_VERSION: &str = "3.31.0";

/// Worker threads parse and walk deep trees too.
const WORKER_STACK: usize = 1 << 30;

/// What `--output` selects, after the defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Text,
    Upstream,
    Short,
    Github,
    Json,
    JsonRich,
}

impl Format {
    fn of(flags: &Flags) -> Format {
        if flags.json {
            return Format::Json;
        }
        match flags.output.as_deref() {
            Some("json") => Format::Json,
            Some("json-rich") => Format::JsonRich,
            Some("short") => Format::Short,
            Some("github") => Format::Github,
            Some("upstream") => Format::Upstream,
            Some(_) => Format::Text,
            None if flags.upstream_bugs => Format::Upstream,
            None => Format::Text,
        }
    }

    /// Whether stdout carries a report for a tool.
    fn is_machine(self) -> bool {
        !matches!(self, Format::Text | Format::Upstream)
    }
}

#[derive(Default)]
pub(crate) struct Flags {
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
    upstream_bugs: bool,
    color: Option<ColorChoice>,
    no_fold: bool,
    divergences: Divergences,
    /// Files or directories to report offenses for. The whole theme is still loaded.
    files: Vec<String>,
    only: Vec<String>,
    skip: Vec<String>,
    severity: Option<Severity>,
}

impl Flags {
    pub(crate) fn timings(&self) -> bool {
        self.timings
    }
}

pub const CHECK_HELP: &str = "\
Check a theme, like `shopify theme check` (theme-check 3.31.0). Never touches the network.

Usage: lq check [options] [files or directories...]

With no file arguments, reports the whole theme. With them, the whole theme is still loaded,
so cross-file checks stay correct, but only offenses in those files are reported and decide
the exit code. Paths are relative to the current directory or to the theme root; a path that
doesn't exist is skipped with a warning. Fits lefthook's {staged_files}.

Filters narrow the report the same way: every check still runs, but only what's left is shown,
decides the exit code and is fixed by --auto-correct. A note on stderr says how much was left out.

  lq check --only OrphanedSnippet          one check
  lq check --only orphan,asset             any check with that in its name, in any case
  lq check --only 'Asset*' --skip AssetPreload
  lq check --severity error snippets/      errors, in these files
  lq check --only asset --list             which enabled checks a filter selects

Options:
      --only <checks>      Report only these checks. A full name, part of a name, or a pattern
                           with *. Comma separated or repeated
      --skip <checks>      Don't report these checks, in the same forms. Wins over --only
      --severity <level>   Report this severity and worse: error, warning or info
  -p, --path <dir>         Theme root [default: current directory]
  -C, --config <file>      Config file, or theme-check:all, :recommended, :theme-app-extension,
                           :nothing [default: .theme-check.yml in the root]
  -o, --output <format>    text, short, github, json, json-rich or upstream [default: text]
                             text       a report for people, on stderr
                             short      path:line:col: severity Check: message, one per line
                             github     annotations for GitHub Actions
                             json       upstream's JSON, unchanged
                             json-rich  everything in the text report, as data
                             upstream   upstream's text layout, on stderr
  -j, --json               Same as --output json
      --color <when>       auto, always or never [default: auto]. Honors NO_COLOR and FORCE_COLOR
      --no-fold            Print every repeat of a problem in full, instead of listing its locations
      --fail-level <level> crash, error, suggestion, style, warning or info [default: error]
  -a, --auto-correct       Apply safe fixes in place
      --upstream-bugs      Reproduce upstream exactly, bugs included: see DIVERGENCES.md. Text
                           output uses upstream's layout too, unless --output says otherwise
      --timings            Print the time spent in each phase to stderr
      --print              Print the resolved config and exit
      --list               List the enabled checks and exit
      --init               Write a .theme-check.yml to the theme root and exit
  -v, --version            Print the theme-check version this targets and exit
  -h, --help               Print this help

Exit code: 0 clean, 1 offenses at or above the fail level or a bad config, 2 bad usage.
text and upstream go to stderr. The formats for tools go to stdout.
Run `lq explain <Check>` for what a check does and how to configure or disable it.
";

const OUTPUTS: [&str; 6] = ["text", "short", "github", "json", "json-rich", "upstream"];

const LEVELS: [&str; 6] = ["crash", "error", "suggestion", "style", "warning", "info"];

/// `a,b` and repeating the flag are the same. An empty name would select every check.
fn push_selectors(into: &mut Vec<String>, name: &str, value: &str) -> Result<(), String> {
    for text in value.split(',').map(str::trim) {
        if text.is_empty() {
            return Err(format!("Flag {name} expects check names, separated by commas"));
        }
        into.push(text.to_string());
    }
    Ok(())
}

pub(crate) fn parse_flags(args: &[String]) -> Result<Flags, String> {
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
                if !OUTPUTS.contains(&v.as_str()) {
                    return Err(format!("Expected --output={v} to be one of: {}", OUTPUTS.join(", ")));
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
            "--only" => push_selectors(&mut f.only, name, &value(&mut i)?)?,
            "--skip" => push_selectors(&mut f.skip, name, &value(&mut i)?)?,
            "--severity" => {
                f.severity = Some(match value(&mut i)?.as_str() {
                    "error" => Severity::Error,
                    "warning" => Severity::Warning,
                    "info" => Severity::Info,
                    v => return Err(format!("Expected --severity={v} to be one of: error, warning, info")),
                });
            }
            "--json" | "-j" => f.json = true,
            "--auto-correct" | "-a" => f.auto_correct = true,
            "--init" => f.init = true,
            "--list" => f.list = true,
            "--print" => f.print = true,
            "--version" | "-v" => f.version = true,
            "--help" | "-h" => f.help = true,
            "--timings" => f.timings = true,
            "--upstream-bugs" => {
                f.upstream_bugs = true;
                f.divergences = Divergences::NONE;
            }
            "--no-fold" => f.no_fold = true,
            "--color" => {
                f.color = Some(match value(&mut i)?.as_str() {
                    "auto" => ColorChoice::Auto,
                    "always" => ColorChoice::Always,
                    "never" => ColorChoice::Never,
                    v => return Err(format!("Expected --color={v} to be one of: auto, always, never")),
                });
            }
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

fn absolute(p: &str) -> String {
    std::path::absolute(p).map_or_else(|_| uri::from_native(p), |a| uri::from_native(&a.to_string_lossy()))
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

fn list_checks(config: &Config, filter: &Filter) {
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
        if !filter.allows(code, severity) {
            continue;
        }
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
/// The absolute theme root that `--path` selects.
pub(crate) fn theme_root(flags: &Flags) -> String {
    let root = absolute(flags.path.as_deref().unwrap_or("."));
    let root = root.strip_suffix('/').filter(|r| !r.is_empty()).map_or(root.clone(), String::from);
    uri::posix_normalize(&root)
}

/// The absolute paths of the file arguments. Empty when there are none, and arguments that don't
/// exist are left out: `check` has already refused those.
pub(crate) fn selected_paths(flags: &Flags, root: &str) -> Vec<String> {
    Selection::new(&flags.files, root).resolved()
}

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
    let filter = match Filter::new(&flags.only, &flags.skip, flags.severity) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let format = Format::of(&flags);
    let json = format.is_machine();
    if json {
        let conflict = [(flags.init, "init"), (flags.version, "version"), (flags.print, "print"), (flags.list, "list")]
            .into_iter()
            .find(|(on, _)| *on);
        if let Some((_, name)) = conflict {
            if format == Format::Json {
                eprintln!("JSON output can't be used with --{name}.");
            } else {
                eprintln!("--output {} can't be used with --{name}.", flags.output.as_deref().unwrap_or("json"));
            }
            return 2;
        }
    }
    let root = theme_root(&flags);
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
            list_checks(&load(config_path.as_deref(), &root)?, &filter);
            return Ok(0);
        }
        run_check(&flags, &filter, format, &root, config_path.as_deref())
    })();
    match result {
        Ok(code) => code,
        Err(e) => {
            if matches!(format, Format::Json | Format::JsonRich) {
                println!("{{\"error\":{{\"type\":\"abort\",\"message\":{}}}}}", quote(&e.message));
            } else if format != Format::Upstream {
                let term = Term::detect(flags.color.unwrap_or(ColorChoice::Auto));
                eprintln!("{} {}", term.paint(style::ERROR, "error"), crate::report::sanitize(&e.message));
                if let Some(next) = e.next_steps {
                    eprintln!("{next}");
                }
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

fn run_check(flags: &Flags, filter: &Filter, format: Format, root: &str, config_path: Option<&str>) -> Result<i32, CliError> {
    let timing = flags.timings || std::env::var_os("LQ_TIMING").is_some();
    let t0 = Instant::now();
    let selection = Selection::new(&flags.files, root);
    if !selection.missing.is_empty() {
        for path in &selection.missing {
            eprintln!("[lq] error: {path} doesn't exist, in this directory or in the theme");
        }
        return Ok(2);
    }
    let mut config = load(config_path, root)?;
    config.divergences = flags.divergences;
    config.hints = matches!(format, Format::Text | Format::JsonRich);
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
        // None of what follows is ever dropped. The process is about to end, which is the fast way
        // to free it, and dropping an AST recurses on nesting depth.
        let sources: ManuallyDrop<Vec<(String, String)>> = ManuallyDrop::new(
            paths
                .par_iter()
                .filter_map(|p| {
                    let source = lq_check::fs::read_utf8_lossy(Path::new(p)).ok()?;
                    Some((uri::file_uri(p), source))
                })
                .collect(),
        );
        let t_read = t.elapsed();

        let t = Instant::now();
        let theme = ManuallyDrop::new(lq_check::parse_theme(&sources));
        let t_parse = t.elapsed();

        let t = Instant::now();
        let deps = ManuallyDrop::new(NodeDependencies::new(&theme, &config, root));
        let mut output = ManuallyDrop::new(lq_check::check(&theme, &config, &*deps));
        let t_checks = t.elapsed();
        let hidden = hidden_errors(&output.offenses, &selection);
        output.offenses.retain(|o| selection.contains(&uri::fs_path(&o.uri)));
        let filtered_note = filter.apply(&mut output.offenses);
        let inspected = sources.iter().filter(|(u, _)| selection.contains(&uri::fs_path(u))).count();
        if let Some(reason) = deps.graph_failure().filter(|_| config.divergences.contains(lq_check::Divergence::GraphFailure)) {
            eprintln!("[lq] warning: the theme graph failed to build, so OrphanedSnippet and ValidScopedCSSClass were skipped: {reason}");
        }

        let t = Instant::now();
        let threshold = flags.fail_level.as_deref().map_or(Some(Severity::Error), fail_threshold);
        let valid = !threshold.is_some_and(|r| output.offenses.iter().any(|o| o.severity <= r));
        let groups = grouped(&output.offenses);
        if format == Format::Upstream {
            eprint!("{}", render_text(root, &groups, &sources, inspected, output.offenses.len()));
        }
        let applied = if flags.auto_correct {
            let keep_going = config.divergences.contains(lq_check::Divergence::AutofixAbort);
            Some(apply_autofix(&sources, &output.offenses, keep_going).map_err(|message| CliError { message, next_steps: None }))
        } else {
            None
        };
        // lq's own reports come after the fixes, so that they can say what they did, and the text
        // one even when a fix failed.
        let mut stdout_text = None;
        if !matches!(format, Format::Upstream | Format::Json) {
            let mut report = Report::new(root, &output.offenses, &sources);
            report.inspected = inspected;
            report.valid = valid;
            report.fail_level = flags.fail_level.as_deref().unwrap_or("error");
            report.fold = !flags.no_fold;
            report.applied = applied.as_ref().and_then(|a| a.as_ref().ok().copied());
            if matches!(format, Format::Text | Format::JsonRich) {
                report.check_fixes();
            }
            if format == Format::Text {
                report.elapsed = t0.elapsed();
                let term = Term::detect(flags.color.unwrap_or(ColorChoice::Auto));
                eprint!("{}", rail::render(&report, &term));
            } else if !matches!(applied, Some(Err(_))) {
                stdout_text = Some(match format {
                    Format::JsonRich => format!("{}\n", machine::json_rich(&report)),
                    Format::Short => machine::short(&report),
                    _ => machine::github(&report),
                });
            }
        }
        if let Some(Err(e)) = applied {
            return Err(e);
        }
        if format == Format::Json {
            stdout_text = Some(format!("{}\n", render_json(valid, &groups)));
        }
        if let Some(text) = stdout_text {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            let _ = lock.write_all(text.as_bytes());
            let _ = lock.flush();
        }
        let t_output = t.elapsed();

        if let Some(note) = hidden_note(hidden) {
            eprintln!("{note}");
        }
        if let Some(note) = filtered_note {
            eprintln!("{note}");
        }
        for note in filter.turned_off_notes(&config.checks) {
            eprintln!("{note}");
        }

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
        Ok(i32::from(!valid))
    })
}

/// The files named on the command line. Empty means everything.
struct Selection {
    files: Vec<String>,
    dirs: Vec<String>,
    /// Arguments that don't exist. Checking nothing must not look like a pass.
    missing: Vec<String>,
    all: bool,
}

impl Selection {
    fn new(args: &[String], root: &str) -> Selection {
        let mut s = Selection { files: Vec::new(), dirs: Vec::new(), missing: Vec::new(), all: args.is_empty() };
        for arg in args {
            let from_cwd = uri::posix_normalize(&absolute(arg));
            let from_root = uri::posix_normalize(&uri::from_native(&Path::new(root).join(arg).to_string_lossy()));
            let Some(path) = [from_cwd, from_root].into_iter().find(|p| Path::new(p).exists()) else {
                s.missing.push(arg.clone());
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

    fn resolved(self) -> Vec<String> {
        self.files.into_iter().chain(self.dirs).collect()
    }
}

/// Errors, and the files they're in, that the file arguments leave out of the report.
fn hidden_errors(offenses: &[Offense], selection: &Selection) -> (usize, usize) {
    let hidden: Vec<&Offense> = offenses
        .iter()
        .filter(|o| o.severity == Severity::Error && !selection.contains(&uri::fs_path(&o.uri)))
        .collect();
    let files: std::collections::HashSet<&str> = hidden.iter().map(|o| o.uri.as_str()).collect();
    (hidden.len(), files.len())
}

/// The whole theme is always linted, so a clean report on named files shouldn't read as a clean theme.
fn hidden_note((errors, files): (usize, usize)) -> Option<String> {
    if errors == 0 {
        return None;
    }
    let errors_word = if errors == 1 { "error" } else { "errors" };
    let files_word = if files == 1 { "file" } else { "files" };
    Some(format!(
        "[lq] note: {errors} {errors_word} in {files} other {files_word} not shown, because only the named files are reported. Run `lq check` with no file arguments to see them."
    ))
}

/// `autofix(theme, offenses)`. Upstream stops at the first file whose fix can't be applied;
/// with `keep_going` that file is skipped with a warning and the rest are still fixed.
///
/// Returns the fixes applied and the files they changed.
fn apply_autofix(sources: &[(String, String)], offenses: &[Offense], keep_going: bool) -> Result<(usize, usize), String> {
    let (mut applied, mut files) = (0, 0);
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
        applied += fixes.len();
        files += 1;
    }
    Ok((applied, files))
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
