//! Port of `@shopify/prettier-plugin-liquid` 1.11.3 and the parts of Prettier 3.9.10 it
//! needs: the doc printer, the embed flow, the JSON printer, config resolution, ignore
//! files and the `--check` / `--write` CLI behaviour.
//!
//! Entry points: [`format_source`] for one document, [`run_fmt`] for the CLI. Formatting
//! recurses on document depth, so call it on a thread with a large stack; see
//! [`STACK_SIZE`] and [`with_big_stack`]. [`run_fmt`] arranges that itself.

pub mod cli;
pub mod config;
pub mod constants;
pub mod doc;
pub mod embed;
pub mod glob;
pub mod ignore;
pub mod json;
pub mod options;
pub mod printer;

pub use cli::{run_debug_fmt, run_fmt, FmtArgs};
pub use config::{resolve_options, ConfigError, ResolvedConfig};
pub use options::*;

use doc::printer::{print_doc_to_string, PrintOptions};
use printer::string::js_trim;
use printer::{Args, Ctx};

/// Stack for formatting threads. Virtual memory only until touched.
pub const STACK_SIZE: usize = 512 * 1024 * 1024;

/// A file Prettier would refuse to format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatError {
    /// The JavaScript error class upstream throws, e.g. `LiquidHTMLParsingError`.
    pub name: String,
    /// The error's message. Its first line matches upstream's for the same input.
    pub message: String,
    /// `(line, column)`, 1-based, where the parser reports one.
    pub loc: Option<(u32, u32)>,
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FormatError {}

/// `ignore`'s case-insensitive match of one path segment.
pub(crate) fn glob_segment(pattern: &[char], segment: &[char]) -> bool {
    glob::segment_matches(pattern, segment, true)
}

const BOM: char = '\u{FEFF}';

/// `guessEndOfLine`.
fn guess_end_of_line(text: &str) -> &'static str {
    match text.find(['\n', '\r']) {
        Some(i) if text.as_bytes()[i] == b'\r' => {
            if text.as_bytes().get(i + 1) == Some(&b'\n') {
                "\r\n"
            } else {
                "\r"
            }
        }
        _ => "\n",
    }
}

/// `formatWithCursor` + `coreFormat` for `parser: liquid-html`.
///
/// Recurses on nesting depth: call on a thread with a stack like [`STACK_SIZE`].
pub fn format_source(source: &str, options: &Options) -> Result<String, FormatError> {
    let (has_bom, text) = match source.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, source),
    };
    let new_line = match options.end_of_line {
        EndOfLine::Lf => "\n",
        EndOfLine::Crlf => "\r\n",
        EndOfLine::Cr => "\r",
        EndOfLine::Auto => guess_end_of_line(text),
    };
    let normalized;
    let text = if text.contains('\r') {
        normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        normalized.as_str()
    } else {
        text
    };

    let mut out = if js_trim(text).is_empty() {
        String::new()
    } else {
        format_normalized(text, options, new_line)?
    };
    if has_bom {
        out.insert(0, BOM);
    }
    Ok(out)
}

fn format_normalized(
    text: &str,
    options: &Options,
    new_line: &'static str,
) -> Result<String, FormatError> {
    let document = lq_syntax::parse_tolerant(text).map_err(|e| FormatError {
        name: e.name.clone(),
        message: e.message.clone(),
        loc: e.loc.map(|l| (l.start.line, l.start.column)),
    })?;
    let mut ctx = Ctx::new(text, options, document.node_count as usize);
    printer::preprocess::preprocess(&mut ctx, &document.root)
        .map_err(|message| FormatError { name: "TypeError".to_string(), message, loc: None })?;
    let mut doc = ctx.print_node(&document.root, Args::default());
    if let Some(message) = ctx.error.borrow_mut().take() {
        return Err(FormatError { name: "Error".to_string(), message, loc: None });
    }
    let print_options = PrintOptions {
        print_width: options.print_width,
        tab_width: options.tab_width,
        use_tabs: options.use_tabs,
        new_line,
    };
    Ok(print_doc_to_string(&mut doc, &print_options, text.len()))
}

/// Runs `f` on a new thread with a [`STACK_SIZE`] stack.
pub fn with_big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(f)
        .expect("spawn formatting thread")
        .join()
        .expect("formatting thread panicked")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(src: &str) -> String {
        format_source(src, &Options::default()).unwrap()
    }

    #[test]
    fn blank_input_formats_to_nothing() {
        assert_eq!(fmt("  \n "), "");
        assert_eq!(fmt("\u{FEFF}"), "\u{FEFF}");
    }

    #[test]
    fn basics() {
        assert_eq!(fmt("<div  a=1 ></div>\n"), "<div a=\"1\"></div>\n");
        assert_eq!(fmt("{{a|b}}"), "{{ a | b }}\n");
        assert_eq!(fmt("{%if a%}b{%endif%}"), "{% if a %}b{% endif %}\n");
    }

    #[test]
    fn crlf_is_normalized() {
        assert_eq!(fmt("<p>\r\n</p>\r\n"), "<p></p>\n");
    }

    #[test]
    fn json_schema() {
        assert_eq!(fmt("{% schema %}\n{\"a\":1}\n{% endschema %}"), "{% schema %}\n{ \"a\": 1 }\n{% endschema %}\n");
    }

    #[test]
    fn parser_errors_surface() {
        let e = format_source("{% toString %}", &Options::default()).unwrap_err();
        assert_eq!(e.message, "Unexpected object: undefined");
    }
}
