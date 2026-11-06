use lq_syntax::{dump, SourceMap};
use std::io::Write;

mod check_cmd;
mod explain;
mod filter;
mod report;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Parsing recurses on nesting depth, and so do the dump and `Drop`.
const STACK_SIZE: usize = 1 << 30;

const HELP: &str = "\
lq: a fast native clone of `shopify theme check` (theme-check 3.31.0) and the Prettier Liquid
plugin (1.11.3). Not affiliated with or endorsed by Shopify.

Usage: lq <command> [options]

Commands:
  check [files...]   Lint a theme. See `lq check --help`.
  fmt [paths...]     Format Liquid files. See `lq fmt --help`.
  explain <check>    What a check does, and how to configure or disable it.

Options:
  -h, --help         Print this help
  -V, --version      Print versions

lq never uses the network.
";

const USAGE: &str = "usage: lq check [options] [files...]   (lq check --help)
       lq fmt [options] [paths...]    (lq fmt --help)
       lq explain <check>
debug: lq debug-check-golden <file> [--show <n>] [--upstream-bugs]
       lq debug-checks
       lq debug-divergences
       lq debug-engine --path <root> [--upstream-bugs]
       lq debug-offenses --path <root> [--upstream-bugs] [--only-divergence <ID>]
       lq debug-visit --files-from <list>
       lq debug-ast [--tolerant] [--liquid-only] [--walk] [--bench] --files-from <list>
       lq debug-lax --cases <file>
       lq debug-fmt --files-from <list> | --cases <file> [options]
       lq debug-embed <list>";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let handle = std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(move || run(&args))
        .expect("spawn parser thread");
    std::process::exit(handle.join().unwrap_or(101));
}

fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("check") => check_cmd::check(&args[1..]),
        Some("fmt") => lq_fmt::cli::fmt_main(&args[1..]),
        Some("explain") => explain::explain(&args[1..]),
        Some("--help" | "-h" | "help") => {
            print!("{HELP}");
            0
        }
        Some("--version" | "-V") => {
            println!("lq {} (theme-check 3.31.0, prettier-plugin-liquid 1.11.3)", env!("CARGO_PKG_VERSION"));
            0
        }
        Some("debug-ast") => debug_ast(&args[1..]),
        Some("debug-lax") => debug_lax(&args[1..]),
        Some("debug-checks") => check_cmd::debug_checks(),
        Some("debug-divergences") => {
            for d in lq_check::Divergence::ALL {
                println!("{}\t{}\t{}", d.id(), d.check(), d.summary());
            }
            0
        }
        Some("debug-engine") => check_cmd::debug_engine(&args[1..]),
        Some("debug-offenses") => check_cmd::debug_offenses(&args[1..]),
        Some("debug-check-golden") => check_cmd::debug_check_golden(&args[1..]),
        Some("debug-fmt") => lq_fmt::cli::debug_fmt_main(&args[1..]),
        Some("debug-embed") => match args.get(1) {
            Some(list) => lq_fmt::cli::run_debug_embed(std::path::Path::new(list)),
            None => {
                eprintln!("usage: lq debug-embed <list>");
                2
            }
        },
        Some("debug-visit") => match files_from(args) {
            Some(files) => check_cmd::debug_visit(&files),
            None => {
                eprintln!("{USAGE}");
                2
            }
        },
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}

fn files_from(args: &[String]) -> Option<Vec<String>> {
    let i = args.iter().position(|a| a == "--files-from")?;
    let list = std::fs::read_to_string(args.get(i + 1)?).ok()?;
    Some(list.lines().filter(|l| !l.is_empty()).map(str::to_string).collect())
}

fn read_source(path: &str) -> std::io::Result<String> {
    // The lossy decode matches `fs.readFileSync(path, 'utf8')` on invalid input.
    Ok(String::from_utf8_lossy(&std::fs::read(path)?).into_owned())
}

fn debug_ast(args: &[String]) -> i32 {
    let Some(files) = files_from(args) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let tolerant = args.iter().any(|a| a == "--tolerant");
    let liquid_only = args.iter().any(|a| a == "--liquid-only");
    let bench = args.iter().any(|a| a == "--bench");
    let walk = args.iter().any(|a| a == "--walk");
    let parse = match (tolerant, liquid_only) {
        (false, false) => lq_syntax::parse_strict,
        (true, false) => lq_syntax::parse_tolerant,
        (false, true) => lq_syntax::parse_liquid_only,
        (true, true) => lq_syntax::parse_tolerant_liquid_only,
    };

    if bench {
        return bench_parse(&files, parse);
    }

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::with_capacity(1 << 20, stdout.lock());
    for path in &files {
        let line = match read_source(path) {
            Err(e) => format!("{{\"error\":{{\"name\":\"IoError\",\"message\":{:?},\"loc\":null}}}}", e.to_string()),
            Ok(source) => match parse(&source) {
                Ok(doc) if walk => dump::walk_line(&doc, &SourceMap::new(&source)),
                Ok(doc) => dump::ast_line(&doc, &SourceMap::new(&source)),
                Err(e) => dump::error_line(&e),
            },
        };
        if out.write_all(line.as_bytes()).is_err() || out.write_all(b"\n").is_err() {
            return 1;
        }
    }
    i32::from(out.flush().is_err())
}

/// Parse-only timing over the files, read into memory first.
fn bench_parse(
    files: &[String],
    parse: for<'a> fn(&'a str) -> Result<lq_syntax::Document<'a>, lq_syntax::ParseError>,
) -> i32 {
    let sources: Vec<String> = files.iter().filter_map(|p| read_source(p).ok()).collect();
    let bytes: usize = sources.iter().map(String::len).sum();
    let mut best = std::time::Duration::MAX;
    let (mut ok, mut err, mut nodes) = (0u64, 0u64, 0u64);
    for _ in 0..5 {
        (ok, err, nodes) = (0, 0, 0);
        let start = std::time::Instant::now();
        for source in &sources {
            match parse(source) {
                Ok(doc) => {
                    ok += 1;
                    nodes += u64::from(doc.node_count);
                }
                Err(_) => err += 1,
            }
        }
        best = best.min(start.elapsed());
    }
    println!(
        "{} files, {bytes} bytes, {ok} parsed, {err} errors, {nodes} nodes: best of 5 {:.2?}",
        sources.len(),
        best
    );
    0
}

/// The file holds NUL-separated records, each `kind\ttag\n` then the markup, used
/// as both markup and source. Kind is one of `variable`, `tag`, `when`.
fn debug_lax(args: &[String]) -> i32 {
    let path = args.iter().position(|a| a == "--cases").and_then(|i| args.get(i + 1));
    let Some(Ok(cases)) = path.map(|p| read_source(p)) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::with_capacity(1 << 20, stdout.lock());
    for content in cases.split('\0') {
        let (header, markup) = content.split_once('\n').unwrap_or((content, ""));
        let (kind, tag) = header.split_once('\t').unwrap_or((header, ""));
        let map = SourceMap::new(markup);
        let end = markup.len() as u32;
        let undefined = "{\"undefined\":true}".to_string();
        let line = match kind {
            "variable" => match lq_syntax::lax_recover_variable(markup, markup, 0) {
                Some(n) => format!("{{\"value\":{}}}", dump::node_json(&n, &map)),
                None => undefined,
            },
            "when" => match lq_syntax::lax_recover_when_values(markup, markup, 0, end) {
                Some(values) => format!(
                    "{{\"value\":{}}}",
                    dump::markup_json(&lq_syntax::Markup::List(values), &map)
                ),
                None => undefined,
            },
            _ => match lq_syntax::lax_recover_tag_markup(tag, markup, markup, 0, end) {
                Ok(Some(m)) => format!("{{\"value\":{}}}", dump::markup_json(&m, &map)),
                Ok(None) => undefined,
                Err(e) => dump::error_line(&e),
            },
        };
        if out.write_all(line.as_bytes()).is_err() || out.write_all(b"\n").is_err() {
            return 1;
        }
    }
    i32::from(out.flush().is_err())
}
