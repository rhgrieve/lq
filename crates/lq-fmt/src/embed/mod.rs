//! Port of `printer/embed.ts`.
//!
//! JSON has a native printer, so its doc nests like Prettier's. JS, TS and CSS go through
//! Biome, which returns strings: those are formatted when the printer reaches them, with
//! the line width reduced by the indent the block sits at.

#[cfg(feature = "biome")]
pub mod biome;

/// Without the `biome` feature nothing parses, so embedded JS, TS and CSS print verbatim.
#[cfg(not(feature = "biome"))]
pub mod biome {
    use super::EmbeddedLine;

    #[derive(Debug)]
    pub struct Parsed;

    pub fn parse(_source: &str, _kind: lq_syntax::RawMarkupKind) -> Option<Parsed> {
        None
    }

    pub fn set_current_file(_display: &str) {}

    pub fn inject_panic(_step: &str) {}

    impl Parsed {
        pub fn format(&self, _options: &crate::Options, _available: usize) -> Option<Vec<EmbeddedLine>> {
            None
        }
    }
}

use crate::concat;
use crate::doc::{dedent_to_root, indent, Doc};
use crate::printer::string::js_trim;
use crate::printer::{Ctx, N};
use lq_syntax::{Node, RawMarkupKind};

/// One line of formatted embedded code.
#[derive(Debug, PartialEq, Eq)]
pub struct EmbeddedLine {
    pub text: String,
    /// Starts inside a multi-line token: printed as after Prettier's `literalline`.
    pub literal: bool,
}

/// Embedded code in the place of `[indent([hardline, body]), hardline]`.
#[derive(Debug)]
pub struct EmbedDoc<'a> {
    pub code: biome::Parsed,
    pub options: &'a crate::Options,
    /// `shouldIndentBody`: otherwise `dedentToRoot`.
    pub indent_body: bool,
    /// What the plain printer gives, used if Biome's formatter fails after parsing.
    pub fallback: Doc<'a>,
}

impl EmbedDoc<'_> {
    /// Formats for `available` columns. `None` means use `fallback`.
    pub fn format(&self, available: usize) -> Option<Vec<EmbeddedLine>> {
        self.code.format(self.options, available)
    }
}

/// How an embed resolved, for reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbedOutcome {
    Formatted,
    /// The language's parser rejected the block; printed verbatim.
    ParseFailed,
    /// No Rust formatter for this kind; printed verbatim.
    NoFormatter,
    /// Not embedded at all (text, html, blank bodies).
    NotApplicable,
}

impl<'a> Ctx<'a> {
    /// The `RawMarkup` case of `embed3`, with Prettier's catch-and-fall-back around it.
    /// `plain` is what `printNode` prints when embedding declines or fails.
    pub fn embed(&self, node: N<'a>, plain: Doc<'a>) -> Doc<'a> {
        let Node::RawMarkup(raw) = node else {
            return plain;
        };
        if js_trim(raw.value).is_empty() {
            return plain;
        }
        match raw.kind {
            RawMarkupKind::Json => match crate::json::text_to_doc(raw.value, self) {
                Some(body) => {
                    if self.should_indent_body(node) {
                        concat![indent(concat![Doc::Hardline, body]), Doc::Hardline]
                    } else {
                        concat![dedent_to_root(concat![Doc::Hardline, body]), Doc::Hardline]
                    }
                }
                None => plain,
            },
            RawMarkupKind::Css | RawMarkupKind::Javascript | RawMarkupKind::Typescript => {
                match biome::parse(raw.value, raw.kind) {
                    Some(code) => Doc::Embed(Box::new(EmbedDoc {
                        code,
                        options: self.options,
                        indent_body: self.should_indent_body(node),
                        fallback: plain,
                    })),
                    None => plain,
                }
            }
            // Markdown has no Rust formatter. Html and text are `null` in `ParserMap`.
            RawMarkupKind::Markdown | RawMarkupKind::Html | RawMarkupKind::Text => plain,
        }
    }
}

/// Classifies what `embed` does with a block, without building docs.
pub fn classify(kind: RawMarkupKind, value: &str, options: &crate::Options) -> EmbedOutcome {
    if js_trim(value).is_empty() {
        return EmbedOutcome::NotApplicable;
    }
    match kind {
        RawMarkupKind::Json => {
            let _ = options;
            if crate::json::parser::parse(value).is_ok() {
                EmbedOutcome::Formatted
            } else {
                EmbedOutcome::ParseFailed
            }
        }
        RawMarkupKind::Css | RawMarkupKind::Javascript | RawMarkupKind::Typescript => {
            if biome::parse(value, kind).is_some() {
                EmbedOutcome::Formatted
            } else {
                EmbedOutcome::ParseFailed
            }
        }
        RawMarkupKind::Markdown => EmbedOutcome::NoFormatter,
        RawMarkupKind::Html | RawMarkupKind::Text => EmbedOutcome::NotApplicable,
    }
}
