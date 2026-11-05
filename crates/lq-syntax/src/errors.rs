//! Errors. `ParseError` is the public shape of upstream's thrown errors;
//! `Failure` is the cheap internal form, which only resolves a location if it
//! reaches the entry point (upstream builds a line table per error).

use crate::ast::Position;
use crate::source_map::SourceMap;

pub const PARSING_ERROR_NAME: &str = "LiquidHTMLParsingError";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineCol {
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loc {
    pub start: LineCol,
    pub end: LineCol,
}

/// Mirrors upstream's `UnclosedNode`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnclosedNode {
    pub node_type: &'static str,
    pub name: String,
    pub block_start_position: Position,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// `e.name`: `LiquidHTMLParsingError`, or `Error` for a non-parsing throw.
    pub name: String,
    pub message: String,
    /// `None` when upstream's error has no `loc` (non-parsing throws).
    pub loc: Option<Loc>,
    pub unclosed: Option<UnclosedNode>,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}

impl std::error::Error for ParseError {}

/// A thrown error inside the parser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// `LiquidHTMLASTParsingError`, with source offsets in bytes.
    Parsing { message: String, start: u32, end: u32 },
    /// Any other `Error`.
    Other { name: &'static str, message: String },
}

pub type PResult<T> = Result<T, Failure>;

impl Failure {
    pub fn parsing(message: impl Into<String>, start: u32, end: u32) -> Self {
        Failure::Parsing { message: message.into(), start, end }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Failure::Other { name: "Error", message: message.into() }
    }

    /// `e.message`.
    pub fn message(&self) -> &str {
        match self {
            Failure::Parsing { message, .. } | Failure::Other { message, .. } => message,
        }
    }

    pub fn into_message(self) -> String {
        match self {
            Failure::Parsing { message, .. } | Failure::Other { message, .. } => message,
        }
    }

    pub fn into_parse_error(self, source: &str) -> ParseError {
        match self {
            Failure::Parsing { message, start, end } => ParseError {
                name: PARSING_ERROR_NAME.to_string(),
                message,
                loc: Some(resolve_loc(source, start, end)),
                unclosed: None,
            },
            Failure::Other { name, message } => {
                ParseError { name: name.to_string(), message, loc: None, unclosed: None }
            }
        }
    }
}

/// The clamping from errors.ts: both indexes clamp into `0..=length-1`, and an
/// empty source (where `fromIndex` is still null) falls back to 1:1.
fn resolve_loc(source: &str, start: u32, end: u32) -> Loc {
    let map = SourceMap::new(source);
    let last = map.utf16_len().saturating_sub(1);
    let at = |byte: u32| {
        let clamped = map.utf16_index(byte.min(source.len() as u32)).min(last);
        match map.line_col_of_utf16(clamped) {
            Some((line, column)) => LineCol { line, column },
            None => LineCol { line: 1, column: 1 },
        }
    };
    Loc { start: at(start), end: at(end) }
}

