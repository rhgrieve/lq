//! `lq all`: `lq check`, then `lq fmt`, from one command.

use crate::check_cmd;
use lq_fmt::cli::{run_fmt_in, FmtArgs};
use std::path::Path;

pub const ALL_HELP: &str = "\
Run `lq check`, then `lq fmt --check`. With --write, apply safe fixes, then format.

Usage: lq all [options] [files or directories...]

With no file arguments, covers the whole theme and every .liquid file under its root. With them,
both commands get those files, and fmt skips any that aren't Liquid. Globs aren't accepted. Fits
lefthook's {staged_files}.

Options:
  -w, --write    Apply check's safe fixes, then rewrite files with fmt
  -h, --help     Print this help

Every other `lq check` option works, and applies to check only: --path, --config, --output,
--color, --no-fold, --fail-level, --only, --skip, --severity, --upstream-bugs and --timings.
--path also sets the directory fmt works in. fmt finds its config and ignore files as usual, so
`ignore:` in .theme-check.yml doesn't affect fmt, and .prettierignore doesn't affect check.

Exit code: the higher of the two commands': 0 clean, 1 offenses or differences, 2 bad usage or
errors. If check exits 2, fmt doesn't run. stdout holds only check's report for tools, so
`--output json` stays valid JSON. Everything from fmt goes to stderr.
";

/// Flags of `lq check` that do something other than check a theme, or that `--write` replaces.
const REJECTED: [(&str, &str); 7] = [
    ("--auto-correct", "use --write"),
    ("-a", "use --write"),
    ("--init", "run `lq check --init`"),
    ("--list", "run `lq check --list`"),
    ("--print", "run `lq check --print`"),
    ("--version", "run `lq --version`"),
    ("-v", "run `lq --version`"),
];

pub fn all(args: &[String]) -> i32 {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let (options, rest) = args.split_at(end);
    if options.iter().any(|a| a == "--help" || a == "-h") {
        print!("{ALL_HELP}");
        return 0;
    }
    if let Some((flag, instead)) = REJECTED.iter().find(|(flag, _)| options.iter().any(|a| a == flag)) {
        eprintln!("`lq all` doesn't take {flag}: {instead}");
        return 2;
    }
    let write = options.iter().any(|a| a == "--write" || a == "-w");
    let mut check_args: Vec<String> =
        options.iter().filter(|a| *a != "--write" && *a != "-w").cloned().collect();
    if write {
        check_args.push("--auto-correct".to_string());
    }
    check_args.extend(rest.iter().cloned());

    let flags = match check_cmd::parse_flags(&check_args) {
        Ok(flags) => flags,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let check_code = check_cmd::check(&check_args);
    if check_code == 2 {
        return 2;
    }

    let root = check_cmd::theme_root(&flags);
    let mut paths = check_cmd::selected_paths(&flags, &root);
    if paths.is_empty() {
        paths.push("**/*.liquid".to_string());
    }
    let fmt_code = run_fmt_in(
        Path::new(&root),
        &FmtArgs {
            paths,
            check: !write,
            write,
            ignore_unknown: true,
            timings: flags.timings(),
            embedded: true,
            ..FmtArgs::default()
        },
    );
    check_code.max(fmt_code)
}
