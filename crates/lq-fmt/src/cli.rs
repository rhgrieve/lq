//! The formatter commands: `debug-fmt` for the parity harness and `fmt`, which mirrors
//! `prettier [--check|--write] [paths]` for Liquid files.

use crate::config::{pathdiff, ConfigSearch};
use crate::glob::Glob;
use crate::ignore::Ignorer;
use crate::options::*;
use crate::{format_source, FormatError, STACK_SIZE};
use rayon::prelude::*;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Parsed `fmt` arguments.
#[derive(Clone, Debug, Default)]
pub struct FmtArgs {
    pub paths: Vec<String>,
    pub check: bool,
    pub write: bool,
    pub list_different: bool,
    pub stdin_filepath: Option<String>,
    pub ignore_path: Vec<PathBuf>,
    pub with_node_modules: bool,
    pub config: Option<PathBuf>,
    pub no_editorconfig: bool,
    pub ignore_unknown: bool,
    pub timings: bool,
}

impl FmtArgs {
    /// Parses what follows `fmt`.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut out = FmtArgs::default();
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            let value = |i: &mut usize| -> Result<String, String> {
                *i += 1;
                args.get(*i).cloned().ok_or_else(|| format!("{a} needs a value"))
            };
            match a {
                "--check" | "-c" => out.check = true,
                "--write" | "-w" => out.write = true,
                "--list-different" | "-l" => out.list_different = true,
                "--stdin-filepath" | "--stdin-file-path" => out.stdin_filepath = Some(value(&mut i)?),
                "--ignore-path" => out.ignore_path.push(PathBuf::from(value(&mut i)?)),
                "--with-node-modules" => out.with_node_modules = true,
                "--config" => out.config = Some(PathBuf::from(value(&mut i)?)),
                "--no-editorconfig" => out.no_editorconfig = true,
                "--ignore-unknown" | "-u" => out.ignore_unknown = true,
                "--timings" => out.timings = true,
                "--" => {
                    out.paths.extend(args[i + 1..].iter().cloned());
                    break;
                }
                s if s.starts_with('-') && s != "-" => return Err(format!("Unknown option {s}")),
                s => out.paths.push(s.to_string()),
            }
            i += 1;
        }
        Ok(out)
    }
}

fn pool() -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .stack_size(STACK_SIZE)
        .build()
        .expect("build formatting thread pool")
}

const ALWAYS_IGNORED_DIRECTORIES: [&str; 5] = [".git", ".sl", ".svn", ".hg", ".jj"];

fn is_ignored_directory_name(name: &str, with_node_modules: bool) -> bool {
    ALWAYS_IGNORED_DIRECTORIES.contains(&name) || (!with_node_modules && name == "node_modules")
}

fn walk(dir: &Path, with_node_modules: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let Ok(ty) = e.file_type() else {
            continue;
        };
        let path = e.path();
        if ty.is_dir() {
            if !is_ignored_directory_name(&e.file_name().to_string_lossy(), with_node_modules) {
                walk(&path, with_node_modules, out);
            }
        } else if ty.is_file() {
            out.push(path);
        }
        // Symbolic links are skipped: `followSymbolicLinks: false`.
    }
}

/// ICU root collation order for ASCII, which `localeCompare` uses: punctuation before
/// digits before letters, case as a tie-breaker with lowercase first.
fn primary_weight(c: char) -> u32 {
    const PUNCTUATION: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";
    if c == ' ' {
        return 1;
    }
    if let Some(i) = PUNCTUATION.find(c) {
        return 10 + i as u32;
    }
    if c.is_ascii_digit() {
        return 100 + c as u32;
    }
    if c.is_ascii_alphabetic() {
        return 200 + c.to_ascii_lowercase() as u32;
    }
    1000 + c as u32
}

fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| s.chars().map(primary_weight).collect::<Vec<_>>();
    key(a).cmp(&key(b)).then_with(|| {
        for (x, y) in a.chars().zip(b.chars()) {
            if x != y {
                // Lowercase sorts before uppercase at the tertiary level.
                return x.is_ascii_uppercase().cmp(&y.is_ascii_uppercase());
            }
        }
        std::cmp::Ordering::Equal
    })
}

fn is_liquid(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "liquid")
}

/// One input of the expansion: a file, plus whether non-Liquid files are skipped silently.
struct Expanded {
    file: PathBuf,
    ignore_unknown: bool,
}

/// `expandPatterns`.
fn expand_patterns(
    cwd: &Path,
    args: &FmtArgs,
    errors: &mut Vec<String>,
) -> Vec<Expanded> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let with_nm = args.with_node_modules;
    let mut negative: Vec<Glob> = Vec::new();
    let mut entries: Vec<(&str, String)> = Vec::new();

    for pattern in &args.paths {
        let absolute = cwd.join(pattern);
        let relative = pathdiff(&absolute, cwd);
        if relative.split('/').any(|d| is_ignored_directory_name(d, with_nm)) {
            continue;
        }
        match std::fs::symlink_metadata(&absolute) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    errors.push(format!("Explicitly specified pattern \"{pattern}\" is a symbolic link."));
                } else if meta.is_file() {
                    entries.push(("file", pattern.clone()));
                } else if meta.is_dir() {
                    entries.push(("dir", pattern.clone()));
                }
            }
            Err(_) => {
                if let Some(rest) = pattern.strip_prefix('!') {
                    negative.push(Glob::new(rest));
                } else {
                    entries.push(("glob", pattern.clone()));
                }
            }
        }
    }

    let is_negated = |path: &Path| {
        let rel = pathdiff(path, cwd);
        negative.iter().any(|g| g.is_match(&rel))
    };

    for (ty, input) in &entries {
        let mut result: Vec<PathBuf> = Vec::new();
        let ignore_unknown = *ty == "dir";
        match *ty {
            "file" => {
                let p = cwd.join(input);
                if !is_negated(&p) {
                    result.push(p);
                }
            }
            "dir" => {
                walk(&cwd.join(input), with_nm, &mut result);
                result.retain(|p| !is_negated(p));
            }
            _ => {
                let glob = Glob::new(input);
                let mut all = Vec::new();
                walk(cwd, with_nm, &mut all);
                all.retain(|p| glob.is_match(&pathdiff(p, cwd)) && !is_negated(p));
                result = all;
            }
        }
        if result.is_empty() {
            let message = match *ty {
                "file" => "Explicitly specified file was ignored due to negative glob patterns",
                "dir" => "No supported files were found in the directory",
                _ => "No files matching the pattern were found",
            };
            errors.push(format!("{message}: \"{input}\"."));
            continue;
        }
        result.sort_by(|a, b| {
            locale_compare(&pathdiff(a, cwd), &pathdiff(b, cwd))
        });
        for file in result {
            if seen.insert(file.clone()) {
                out.push(Expanded { file, ignore_unknown });
            }
        }
    }
    out
}

enum FileResult {
    Skipped,
    /// With the milliseconds formatting took, which `--write` prints.
    Unchanged(u128),
    Different(String, u128),
    Failed(String),
}

fn first_line_error(display: &str, e: &FormatError) -> String {
    match e.loc {
        Some((line, column)) => format!("{display}: {}: {} ({line}:{column})", e.name, e.message),
        None => format!("{display}: {}: {}", e.name, e.message),
    }
}

/// The `prettier` CLI's `main`. Returns the process exit code.
pub fn run_fmt(args: &FmtArgs) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[error] {e}");
            return 2;
        }
    };
    let ignore_paths = if args.ignore_path.is_empty() {
        vec![PathBuf::from(".gitignore"), PathBuf::from(".prettierignore")]
    } else {
        args.ignore_path.clone()
    };
    let ignorer = match Ignorer::load(&cwd, &ignore_paths, args.with_node_modules) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("[error] {e}");
            return 2;
        }
    };
    let mut search = ConfigSearch::new();
    search.explicit_config = args.config.as_ref().map(|c| cwd.join(c));
    search.use_editorconfig = !args.no_editorconfig;

    if args.paths.is_empty() {
        return run_stdin(args, &cwd, &ignorer, &search);
    }

    if args.check {
        println!("Checking formatting...");
    }

    let t0 = std::time::Instant::now();
    let mut errors = Vec::new();
    let expanded = expand_patterns(&cwd, args, &mut errors);
    let t_discovery = t0.elapsed();
    let mut exit_code = 0;
    for e in &errors {
        eprintln!("[error] {e}");
        exit_code = 2;
    }
    if expanded.is_empty() && errors.is_empty() {
        eprintln!("[error] No matching files. Patterns: {}", args.paths.join(" "));
        exit_code = 2;
    }

    let results: Vec<FileResult> = pool().install(|| {
        expanded
            .par_iter()
            .map(|e| {
                let display = pathdiff(&e.file, &cwd);
                if ignorer.is_ignored(&e.file) && (args.check || args.write || args.list_different) {
                    return FileResult::Skipped;
                }
                if !is_liquid(&e.file) {
                    if e.ignore_unknown || args.ignore_unknown {
                        return FileResult::Skipped;
                    }
                    return FileResult::Failed(format!(
                        "No parser could be inferred for file \"{}\".",
                        e.file.display()
                    ));
                }
                let options = match search.options_for(&e.file) {
                    Ok(o) => o,
                    Err(err) => return FileResult::Failed(err.message),
                };
                let input = match std::fs::read(&e.file) {
                    Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                    Err(err) => {
                        return FileResult::Failed(format!(
                            "Unable to read file \"{display}\":\n{err}"
                        ))
                    }
                };
                if ignorer.is_ignored(&e.file) {
                    return FileResult::Different(input, 0);
                }
                let start = std::time::Instant::now();
                crate::embed::biome::set_current_file(&display);
                let result = format_source(&input, &options);
                let ms = start.elapsed().as_millis();
                match result {
                    Ok(out) if out == input => FileResult::Unchanged(ms),
                    Ok(out) => FileResult::Different(out, ms),
                    Err(err) => FileResult::Failed(first_line_error(&display, &err)),
                }
            })
            .collect()
    });

    if args.timings {
        eprintln!(
            "[lq] timing: discovery {t_discovery:.2?} ({} files), format {:.2?}",
            expanded.len(),
            t0.elapsed() - t_discovery
        );
    }

    let quiet = args.check || args.list_different;
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    let mut unformatted = 0;
    let mut with_error = 0;
    for (e, result) in expanded.iter().zip(results) {
        let display = pathdiff(&e.file, &cwd);
        match result {
            FileResult::Skipped => {}
            FileResult::Failed(message) => {
                eprintln!("[error] {message}");
                exit_code = 2;
                with_error += 1;
            }
            FileResult::Unchanged(ms) => {
                if args.write && !quiet {
                    let _ = writeln!(stdout, "{display} {ms}ms (unchanged)");
                } else if !args.write && !quiet {
                    if let Ok(b) = std::fs::read(&e.file) {
                        let _ = stdout.write_all(&b);
                    }
                }
            }
            FileResult::Different(out, ms) => {
                if args.write {
                    if !quiet {
                        let _ = writeln!(stdout, "{display} {ms}ms");
                    }
                    if let Err(err) = std::fs::write(&e.file, &out) {
                        eprintln!("[error] Unable to write file \"{display}\":\n{err}");
                        exit_code = 2;
                    }
                } else if !quiet {
                    let _ = stdout.write_all(out.as_bytes());
                }
                if args.check {
                    eprintln!("[warn] {display}");
                } else if args.list_different {
                    let _ = writeln!(stdout, "{display}");
                }
                unformatted += 1;
            }
        }
    }

    if args.check {
        if with_error > 0 {
            let files = if with_error == 1 {
                "the above file".to_string()
            } else {
                format!("{with_error} files")
            };
            let _ = writeln!(stdout, "Error occurred when checking code style in {files}.");
        } else if unformatted == 0 {
            let _ = writeln!(stdout, "All matched files use Prettier code style!");
        } else {
            let files = if unformatted == 1 {
                "the above file".to_string()
            } else {
                format!("{unformatted} files")
            };
            if args.write {
                eprintln!("[warn] Code style issues fixed in {files}.");
            } else {
                eprintln!(
                    "[warn] Code style issues found in {files}. Run Prettier with --write to fix."
                );
            }
        }
    }
    if (args.check || args.list_different) && unformatted > 0 && exit_code == 0 && !args.write {
        exit_code = 1;
    }
    exit_code
}

fn run_stdin(args: &FmtArgs, cwd: &Path, ignorer: &Ignorer, search: &ConfigSearch) -> i32 {
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("[error] {e}");
        return 2;
    }
    let absolute = args.stdin_filepath.as_ref().map(|p| cwd.join(p));
    if let Some(a) = &absolute {
        if ignorer.is_ignored(a) {
            print!("{input}");
            return 0;
        }
    }
    let options = match &absolute {
        Some(a) => match search.options_for(a) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("[error] {}", e.message);
                return 2;
            }
        },
        None => Options::default(),
    };
    let display = args.stdin_filepath.clone().unwrap_or_else(|| "stdin".to_string());
    let result = std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(STACK_SIZE)
            .spawn_scoped(s, || format_source(&input, &options))
            .expect("spawn formatting thread")
            .join()
            .expect("formatting thread panicked")
    });
    match result {
        Ok(out) => {
            if args.check || args.list_different {
                if out != input {
                    println!("(stdin)");
                    return 1;
                }
                return 0;
            }
            print!("{out}");
            0
        }
        Err(e) => {
            eprintln!("[error] {}", first_line_error(&display, &e));
            2
        }
    }
}

/// Options for `debug-fmt`.
#[derive(Clone, Debug)]
pub struct DebugFmtArgs {
    pub input: DebugInput,
    pub options: Options,
}

#[derive(Clone, Debug)]
pub enum DebugInput {
    /// A file with one path per line, all formatted with `options`.
    FilesFrom(PathBuf),
    /// JSON lines with per-case options layered over `options`.
    Cases(PathBuf),
}

fn json_string(s: &str, out: &mut String) {
    use std::fmt::Write as _;
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `debug-fmt`: one JSON line per listed file, in order.
pub fn run_debug_fmt(args: &DebugFmtArgs) -> i32 {
    let files_from = match &args.input {
        DebugInput::FilesFrom(f) => f,
        DebugInput::Cases(c) => return run_debug_fmt_cases(c, &args.options),
    };
    let list = match std::fs::read_to_string(files_from) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let files: Vec<&str> = list.lines().filter(|l| !l.is_empty()).collect();
    let lines: Vec<String> = pool().install(|| {
        files
            .par_iter()
            .map(|f| {
                let mut line = String::new();
                match std::fs::read(f) {
                    Ok(bytes) => match String::from_utf8(bytes) {
                        Ok(src) => match format_source(&src, &args.options) {
                            Ok(out) => {
                                line.push_str("{\"out\":");
                                json_string(&out, &mut line);
                                line.push('}');
                            }
                            Err(e) => {
                                line.push_str("{\"error\":");
                                json_string(e.message.lines().next().unwrap_or(""), &mut line);
                                line.push('}');
                            }
                        },
                        Err(e) => {
                            line.push_str("{\"error\":");
                            json_string(&e.to_string(), &mut line);
                            line.push('}');
                        }
                    },
                    Err(e) => {
                        line.push_str("{\"error\":");
                        json_string(&e.to_string(), &mut line);
                        line.push('}');
                    }
                }
                line
            })
            .collect()
    });
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    for l in lines {
        let _ = writeln!(stdout, "{l}");
    }
    0
}

fn parse_case_options(case: &serde_json::Value, base: &Options) -> Options {
    let mut options = base.clone();
    if let Some(serde_json::Value::Object(m)) = case.get("options") {
        // Fixture option names can be anything their messages happen to contain; bad values
        // are left at their defaults, and upstream's run decides whether that fixture counts.
        let _ = crate::config::apply_options(m, &mut options, Path::new("<case>"));
    }
    options
}

/// `debug-fmt --cases`: each line is `{"file": path, "options": {prettier options}}`.
pub fn run_debug_fmt_cases(cases: &Path, base: &Options) -> i32 {
    let list = match std::fs::read_to_string(cases) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let entries: Vec<serde_json::Value> =
        list.lines().filter(|l| !l.is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect();
    let lines: Vec<String> = pool().install(|| {
        entries
            .par_iter()
            .map(|case| {
                let file = case.get("file").and_then(|f| f.as_str()).unwrap_or("");
                let options = parse_case_options(case, base);
                let source = std::fs::read_to_string(file).unwrap_or_default();
                let mut line = String::new();
                match format_source(&source, &options) {
                    Ok(out) => {
                        line.push_str("{\"out\":");
                        json_string(&out, &mut line);
                        line.push('}');
                    }
                    Err(e) => {
                        line.push_str("{\"error\":");
                        json_string(e.message.lines().next().unwrap_or(""), &mut line);
                        line.push('}');
                    }
                }
                line
            })
            .collect()
    });
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    for l in lines {
        let _ = writeln!(stdout, "{l}");
    }
    0
}

/// `debug-embed`: formats embedded blocks stand-alone. Each line of the list is
/// `{"file": path, "kind": css|javascript|typescript, "width": n, "options": {...}}`.
pub fn run_debug_embed(list: &Path) -> i32 {
    use lq_syntax::RawMarkupKind;
    let list = match std::fs::read_to_string(list) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let entries: Vec<serde_json::Value> =
        list.lines().filter(|l| !l.is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect();
    let lines: Vec<String> = pool().install(|| {
        entries
            .par_iter()
            .map(|case| {
                let path = case.get("file").and_then(|f| f.as_str()).unwrap_or("");
                let width = case.get("width").and_then(|w| w.as_u64()).unwrap_or(100) as usize;
                let options = parse_case_options(case, &Options::default());
                let source = std::fs::read_to_string(path).unwrap_or_default();
                let kind = match case.get("kind").and_then(|k| k.as_str()).unwrap_or("") {
                    "css" => RawMarkupKind::Css,
                    "typescript" => RawMarkupKind::Typescript,
                    _ => RawMarkupKind::Javascript,
                };
                let mut line = String::new();
                match crate::embed::biome::parse(&source, kind).and_then(|c| c.format(&options, width)) {
                    Some(lines) => {
                        let text: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
                        line.push_str("{\"out\":");
                        json_string(&text.join("\n"), &mut line);
                        line.push_str(",\"literal\":[");
                        let literal: Vec<String> = lines
                            .iter()
                            .enumerate()
                            .filter(|(_, l)| l.literal)
                            .map(|(i, _)| i.to_string())
                            .collect();
                        line.push_str(&literal.join(","));
                        line.push_str("]}");
                    }
                    None => line.push_str("{\"error\":\"parse\"}"),
                }
                line
            })
            .collect()
    });
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    for l in lines {
        let _ = writeln!(stdout, "{l}");
    }
    0
}

fn parse_debug_fmt(args: &[String]) -> Result<DebugFmtArgs, String> {
    let mut files_from = None;
    let mut cases = None;
    let mut options = Options::default();
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        i += 1;
        // Boolean flags take an explicit true or false.
        let value = args.get(i).ok_or_else(|| format!("{flag} needs a value"))?.as_str();
        i += 1;
        let as_bool = || match value {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(format!("{flag}: expected true or false")),
        };
        let as_usize = || value.parse::<usize>().map_err(|e| format!("{flag}: {e}"));
        match flag {
            "--files-from" => files_from = Some(PathBuf::from(value)),
            // Test hook: `parse` or `format`, proves the panic guard in a release build.
            "--inject-biome-panic" => crate::embed::biome::inject_panic(value),
            "--cases" => cases = Some(PathBuf::from(value)),
            "--print-width" => options.print_width = as_usize()?,
            "--tab-width" => options.tab_width = as_usize()?,
            "--use-tabs" => options.use_tabs = as_bool()?,
            "--single-quote" => options.single_quote = as_bool()?,
            "--bracket-same-line" => options.bracket_same_line = as_bool()?,
            "--single-attribute-per-line" => options.single_attribute_per_line = as_bool()?,
            "--liquid-single-quote" => options.liquid_single_quote = as_bool()?,
            "--embedded-single-quote" => options.embedded_single_quote = as_bool()?,
            "--single-line-link-tags" => options.single_line_link_tags = as_bool()?,
            "--indent-schema" => options.indent_schema = as_bool()?,
            "--liquid-doc-param-dash" => options.liquid_doc_param_dash = as_bool()?,
            "--embedded" => {
                options.embedded_language_formatting = match value {
                    "auto" => EmbeddedLanguageFormatting::Auto,
                    "off" => EmbeddedLanguageFormatting::Off,
                    _ => return Err("--embedded: expected auto or off".to_string()),
                }
            }
            "--html-whitespace-sensitivity" => {
                options.html_whitespace_sensitivity = match value {
                    "css" => HtmlWhitespaceSensitivity::Css,
                    "strict" => HtmlWhitespaceSensitivity::Strict,
                    "ignore" => HtmlWhitespaceSensitivity::Ignore,
                    _ => return Err(format!("{flag}: bad value")),
                }
            }
            "--capture-whitespace-sensitivity" => {
                options.capture_whitespace_sensitivity = match value {
                    "strict" => CaptureWhitespaceSensitivity::Strict,
                    "ignore" => CaptureWhitespaceSensitivity::Ignore,
                    _ => return Err(format!("{flag}: bad value")),
                }
            }
            "--end-of-line" => {
                options.end_of_line = match value {
                    "lf" => EndOfLine::Lf,
                    "crlf" => EndOfLine::Crlf,
                    "cr" => EndOfLine::Cr,
                    "auto" => EndOfLine::Auto,
                    _ => return Err(format!("{flag}: bad value")),
                }
            }
            _ => return Err(format!("Unknown option {flag}")),
        }
    }
    Ok(DebugFmtArgs {
        input: match (files_from, cases) {
            (Some(f), _) => DebugInput::FilesFrom(f),
            (None, Some(c)) => DebugInput::Cases(c),
            _ => return Err("--files-from or --cases is required".to_string()),
        },
        options,
    })
}

/// `lq debug-fmt ...`.
pub fn debug_fmt_main(args: &[String]) -> i32 {
    match parse_debug_fmt(args) {
        Ok(a) => run_debug_fmt(&a),
        Err(e) => {
            eprintln!("{e}");
            2
        }
    }
}

/// Help for `lq fmt`.
pub const FMT_HELP: &str = "\
Format Liquid files, like `prettier` with @shopify/prettier-plugin-liquid 1.11.3. Embedded CSS,
JS and TS go through Biome: see the README for where that differs. Never touches the network.

Usage: lq fmt [options] [files, directories or globs...]

With no paths, formats stdin to stdout. With paths and neither --check nor --write, prints the
formatted files. Reads .prettierrc, .editorconfig, .prettierignore and .gitignore.

Options:
  -c, --check                Report files that would change; exit 1 if any
  -w, --write                Rewrite files in place
  -l, --list-different       Print the paths of files that would change; exit 1 if any
  -u, --ignore-unknown       Skip files that aren't Liquid instead of failing on them
      --stdin-filepath <p>   Treat stdin as this path, for config and ignore lookups
      --ignore-path <file>   Ignore file(s) to read [default: .gitignore, .prettierignore]
      --config <file>        Use this config instead of searching for one
      --no-editorconfig      Don't read .editorconfig
      --with-node-modules    Process files in node_modules
      --timings              Print the time spent in each phase to stderr
  -h, --help                 Print this help

Exit code: 0 clean, 1 --check or --list-different found differences, 2 errors.
For lefthook's {staged_files}, use --ignore-unknown or a `glob:` so non-Liquid files are skipped.
";

/// `lq fmt ...`.
pub fn fmt_main(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{FMT_HELP}");
        return 0;
    }
    let parsed = match FmtArgs::parse(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[error] {e}");
            return 2;
        }
    };
    if parsed.paths.is_empty() && std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        eprint!("{FMT_HELP}");
        return 2;
    }
    run_fmt(&parsed)
}
