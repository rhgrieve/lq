//! Biome as the JS, TS and CSS formatter for embedded code.

use super::EmbeddedLine;
use crate::options::{ArrowParens, ObjectWrap, Options, QuoteProps, TrailingComma};
use biome_css_formatter::context::CssFormatOptions;
use biome_css_parser::{parse_css, CssParserOptions};
use biome_css_syntax::CssSyntaxNode;
use biome_formatter::{
    BracketSameLine, BracketSpacing, Expand, IndentStyle, IndentWidth, LineWidth, QuoteStyle,
};
use biome_js_formatter::context::{
    trailing_commas::TrailingCommas, ArrowParentheses, JsFormatOptions, QuoteProperties, Semicolons,
};
use biome_js_parser::{parse as parse_js, JsParserOptions};
use biome_js_syntax::JsSyntaxNode;
use biome_languages::{CssFileSource, JsFileSource};
use biome_rowan::{Direction, TextRange};
use lq_syntax::RawMarkupKind;
use std::cell::{Cell, RefCell};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Once;

thread_local! {
    /// Inside [`guarded`]: the panic hook stays quiet, since a warning follows.
    static IN_GUARD: Cell<bool> = const { Cell::new(false) };
    /// The file being formatted on this thread, for the warning.
    static CURRENT_FILE: RefCell<String> = const { RefCell::new(String::new()) };
}

static INJECT_PANIC: AtomicU8 = AtomicU8::new(0);
static INSTALL_HOOK: Once = Once::new();

/// Names the file for a warning from this thread.
pub fn set_current_file(display: &str) {
    CURRENT_FILE.with(|f| {
        let mut f = f.borrow_mut();
        f.clear();
        f.push_str(display);
    });
}

/// Test hook: makes Biome's `parse` or `format` step panic.
pub fn inject_panic(step: &str) {
    let value = match step {
        "parse" => 1,
        "format" => 2,
        _ => 0,
    };
    INJECT_PANIC.store(value, Ordering::Relaxed);
}

/// Runs a call into Biome. A panic means `None`, which prints the block verbatim, as Prettier
/// does for a block it can't parse, and a warning on stderr. It must not end the run.
fn guarded<T>(step: u8, what: &str, f: impl FnOnce() -> Option<T>) -> Option<T> {
    INSTALL_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !IN_GUARD.with(Cell::get) {
                previous(info);
            }
        }));
    });
    IN_GUARD.with(|g| g.set(true));
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        assert!(INJECT_PANIC.load(Ordering::Relaxed) != step, "injected Biome panic");
        f()
    }));
    IN_GUARD.with(|g| g.set(false));
    result.unwrap_or_else(|_| {
        let file = CURRENT_FILE.with(|f| f.borrow().clone());
        let location = if file.is_empty() { String::new() } else { format!(" in {file}") };
        eprintln!("[lq] warning: Biome panicked on a {what} block{location}; printing it verbatim");
        None
    })
}

/// A parsed block. Syntax trees are `Rc`-based, so a `Parsed` stays on its thread.
pub enum Parsed {
    Js { syntax: JsSyntaxNode, source_type: JsFileSource },
    Css { syntax: CssSyntaxNode },
}

impl std::fmt::Debug for Parsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Parsed::Js { .. } => "Parsed::Js",
            Parsed::Css { .. } => "Parsed::Css",
        })
    }
}

fn try_parse_js(source: &str, source_type: JsFileSource) -> Option<Parsed> {
    let parsed = parse_js(source, source_type, JsParserOptions::default());
    if parsed.has_errors() {
        return None;
    }
    Some(Parsed::Js { syntax: parsed.syntax(), source_type })
}

/// Parses a block as Prettier's chosen parser would see it. `None` when Biome rejects it.
///
/// Prettier's `babel` parser uses `sourceType: module` with the `jsx` plugin and tolerates
/// strict-mode errors (octal literals, `with`, `delete x`, ...), so a block Biome rejects
/// as a module gets a second try as a script. Its `typescript` parser tries with JSX, then
/// without.
pub fn parse(source: &str, kind: RawMarkupKind) -> Option<Parsed> {
    guarded(1, kind_name(kind), || parse_unguarded(source, kind))
}

fn kind_name(kind: RawMarkupKind) -> &'static str {
    match kind {
        RawMarkupKind::Css => "CSS",
        RawMarkupKind::Javascript => "JavaScript",
        RawMarkupKind::Typescript => "TypeScript",
        _ => "embedded",
    }
}

fn parse_unguarded(source: &str, kind: RawMarkupKind) -> Option<Parsed> {
    match kind {
        RawMarkupKind::Javascript => {
            try_parse_js(source, JsFileSource::jsx())
                .or_else(|| try_parse_js(source, JsFileSource::js_script()))
        }
        RawMarkupKind::Typescript => try_parse_js(source, JsFileSource::tsx())
            .or_else(|| try_parse_js(source, JsFileSource::ts())),
        RawMarkupKind::Css => {
            let parsed = parse_css(source, CssFileSource::css(), CssParserOptions::default());
            if parsed.has_errors() {
                return None;
            }
            Some(Parsed::Css { syntax: parsed.syntax() })
        }
        _ => None,
    }
}

fn line_width(available: usize) -> LineWidth {
    // Biome's range is 1..=320.
    LineWidth::try_from(available.clamp(1, 320) as u16).unwrap_or_default()
}

fn indent_style(options: &Options) -> IndentStyle {
    if options.use_tabs {
        IndentStyle::Tab
    } else {
        IndentStyle::Space
    }
}

fn indent_width(options: &Options) -> IndentWidth {
    IndentWidth::try_from(options.tab_width.clamp(0, 24) as u8).unwrap_or_default()
}

fn quote_style(single: bool) -> QuoteStyle {
    if single {
        QuoteStyle::Single
    } else {
        QuoteStyle::Double
    }
}

impl Parsed {
    /// Formats for `available` columns. `None` on an internal Biome error.
    pub fn format(&self, options: &Options, available: usize) -> Option<Vec<EmbeddedLine>> {
        let what = match self {
            Parsed::Js { .. } => "JavaScript",
            Parsed::Css { .. } => "CSS",
        };
        guarded(2, what, || self.format_unguarded(options, available))
    }

    fn format_unguarded(&self, options: &Options, available: usize) -> Option<Vec<EmbeddedLine>> {
        let (code, literal_ranges) = match self {
            Parsed::Js { syntax, source_type } => {
                let js_options = JsFormatOptions::new(*source_type)
                    .with_indent_style(indent_style(options))
                    .with_indent_width(indent_width(options))
                    .with_line_width(line_width(available))
                    .with_quote_style(quote_style(options.embedded_single_quote))
                    .with_jsx_quote_style(quote_style(options.jsx_single_quote))
                    .with_semicolons(if options.semi {
                        Semicolons::Always
                    } else {
                        Semicolons::AsNeeded
                    })
                    .with_trailing_commas(match options.trailing_comma {
                        TrailingComma::All => TrailingCommas::All,
                        TrailingComma::Es5 => TrailingCommas::Es5,
                        TrailingComma::None => TrailingCommas::None,
                    })
                    .with_arrow_parentheses(match options.arrow_parens {
                        ArrowParens::Always => ArrowParentheses::Always,
                        ArrowParens::Avoid => ArrowParentheses::AsNeeded,
                    })
                    .with_bracket_spacing(BracketSpacing::from(options.bracket_spacing))
                    .with_bracket_same_line(BracketSameLine::from(options.bracket_same_line))
                    .with_quote_properties(match options.quote_props {
                        QuoteProps::Preserve => QuoteProperties::Preserve,
                        QuoteProps::AsNeeded | QuoteProps::Consistent => QuoteProperties::AsNeeded,
                    })
                    .with_expand(match options.object_wrap {
                        ObjectWrap::Preserve => Expand::Auto,
                        ObjectWrap::Collapse => Expand::Never,
                    });
                let formatted =
                    biome_js_formatter::format_node(js_options, syntax, vec![]).ok()?;
                let code = formatted.print().ok()?.as_code().to_string();
                let ranges = js_literal_ranges(&code, *source_type);
                (code, ranges)
            }
            Parsed::Css { syntax } => {
                let css_options = CssFormatOptions::default()
                    .with_indent_style(indent_style(options))
                    .with_indent_width(indent_width(options))
                    .with_line_width(line_width(available))
                    .with_quote_style(quote_style(options.embedded_single_quote));
                let formatted = biome_css_formatter::format_node(css_options, syntax).ok()?;
                let code = formatted.print().ok()?.as_code().to_string();
                restore_css_comments(code, syntax)
            }
        };
        Some(split_lines(&code, &literal_ranges))
    }
}

fn split_lines(code: &str, literal_ranges: &[TextRange]) -> Vec<EmbeddedLine> {
    // `stripTrailingHardline`.
    let code = code.trim_end_matches('\n');
    let mut lines = Vec::new();
    let mut offset = 0u32;
    let mut range = literal_ranges.iter().peekable();
    for (i, text) in code.split('\n').enumerate() {
        let mut literal = false;
        if i > 0 {
            // The newline before this line.
            let newline = offset - 1;
            while range.peek().is_some_and(|r| u32::from(r.end()) <= newline) {
                range.next();
            }
            literal = range
                .peek()
                .is_some_and(|r| u32::from(r.start()) <= newline && newline < u32::from(r.end()));
        }
        lines.push(EmbeddedLine { text: text.to_string(), literal });
        offset += text.len() as u32 + 1;
    }
    lines
}

/// A block comment's lines all start with `*`: Prettier re-indents it (`isIndentable`).
fn is_indentable_block_comment(text: &str) -> bool {
    let inner = text.strip_prefix("/*").and_then(|t| t.strip_suffix("*/")).unwrap_or(text);
    format!("*{inner}*").split('\n').all(|l| l.trim_start().starts_with('*'))
}

/// Multi-line tokens of the formatted code whose inner newlines Prettier prints literally:
/// template chunks, strings with line continuations, and non-indentable block comments.
fn js_literal_ranges(code: &str, source_type: JsFileSource) -> Vec<TextRange> {
    if !code.contains(['`', '\\']) && !code.contains("/*") {
        return Vec::new();
    }
    let parsed = parse_js(code, source_type, JsParserOptions::default());
    let mut out = Vec::new();
    for token in parsed.syntax().descendants_tokens(Direction::Next) {
        for piece in token.leading_trivia().pieces().chain(token.trailing_trivia().pieces()) {
            if piece.is_comments() {
                let text = piece.text();
                if text.contains('\n') && !is_indentable_block_comment(text) {
                    out.push(piece.text_range());
                }
            }
        }
        if token.text_trimmed().contains('\n') {
            out.push(token.text_trimmed_range());
        }
    }
    out.sort_by_key(|r| r.start());
    out
}

/// Prettier prints CSS comments as their source text, so inner lines keep their source
/// columns, while Biome re-indents them. Comments come out in source order, so the n-th
/// comment of the output is restored from the n-th of the source. Returns the code and
/// the ranges of multi-line comments, whose inner lines are literal.
fn restore_css_comments(code: String, source: &CssSyntaxNode) -> (String, Vec<TextRange>) {
    if !code.contains("/*") {
        return (code, Vec::new());
    }
    fn comments(root: &CssSyntaxNode) -> Vec<(TextRange, String)> {
        let mut out = Vec::new();
        for token in root.descendants_tokens(Direction::Next) {
            for piece in token.leading_trivia().pieces().chain(token.trailing_trivia().pieces()) {
                if piece.is_comments() {
                    out.push((piece.text_range(), piece.text().to_string()));
                }
            }
        }
        out
    }
    let parsed = parse_css(&code, CssFileSource::css(), CssParserOptions::default());
    let output_comments = comments(&parsed.syntax());
    let source_comments = comments(source);
    let same_shape = output_comments.len() == source_comments.len();

    let mut rebuilt = String::with_capacity(code.len());
    let mut ranges = Vec::new();
    let mut last = 0usize;
    for (i, (range, text)) in output_comments.iter().enumerate() {
        let (start, end) = (u32::from(range.start()) as usize, u32::from(range.end()) as usize);
        rebuilt.push_str(&code[last..start]);
        let replacement = if same_shape { source_comments[i].1.as_str() } else { text.as_str() };
        let new_start = rebuilt.len() as u32;
        rebuilt.push_str(replacement);
        if replacement.contains('\n') {
            ranges.push(TextRange::new(new_start.into(), (rebuilt.len() as u32).into()));
        }
        last = end;
    }
    rebuilt.push_str(&code[last..]);
    (rebuilt, ranges)
}
