//! `SourceCode` and `toSourceCode`.

use lq_json::{to_json_node, uri, JsonNode, JsonParseFailure};
use lq_syntax::{Document, Node, ParseError};
use rayon::prelude::*;

pub use crate::api::SourceCodeType;

/// The parsed form of a file. A parse failure upstream is an `Error` in `ast`.
#[derive(Debug)]
pub enum Ast<'a> {
    Liquid(Result<Document<'a>, ParseError>),
    Json(Result<JsonNode<'a>, JsonParseFailure>),
}

/// `SourceCode`.
#[derive(Debug)]
pub struct SourceCode<'a> {
    /// Normalized.
    pub uri: String,
    pub source: &'a str,
    pub ast: Ast<'a>,
}

/// `Theme`: the files in discovery order.
pub type Theme<'a> = Vec<SourceCode<'a>>;

impl<'a> SourceCode<'a> {
    pub fn kind(&self) -> SourceCodeType {
        match self.ast {
            Ast::Liquid(_) => SourceCodeType::LiquidHtml,
            Ast::Json(_) => SourceCodeType::Json,
        }
    }

    /// The Liquid AST. Panics on a JSON file: a Liquid check never sees one.
    pub fn liquid_ast(&'a self) -> Result<&'a Node<'a>, &'a ParseError> {
        match &self.ast {
            Ast::Liquid(Ok(doc)) => Ok(&doc.root),
            Ast::Liquid(Err(e)) => Err(e),
            Ast::Json(_) => panic!("liquid_ast on a JSON file"),
        }
    }

    /// The JSON AST. Panics on a Liquid file: a JSON check never sees one.
    pub fn json_ast(&'a self) -> Result<&'a JsonNode<'a>, &'a JsonParseFailure> {
        match &self.ast {
            Ast::Json(Ok(node)) => Ok(node),
            Ast::Json(Err(e)) => Err(e),
            Ast::Liquid(_) => panic!("json_ast on a Liquid file"),
        }
    }
}

/// `toSourceCode(uri, source)`.
pub fn to_source_code<'a>(u: &str, source: &'a str) -> SourceCode<'a> {
    let ast = if u.ends_with(".liquid") {
        Ast::Liquid(lq_syntax::parse_strict(source))
    } else {
        Ast::Json(to_json_node(source))
    };
    SourceCode { uri: uri::normalize(u), source, ast }
}

/// Parses all files in parallel, keeping their order.
pub fn parse_theme<'a>(sources: &'a [(String, String)]) -> Theme<'a> {
    sources.par_iter().map(|(u, s)| to_source_code(u, s)).collect()
}
