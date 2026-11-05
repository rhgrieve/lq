//! Liquid + HTML parser, ported from @shopify/liquid-html-parser 2.11.0.
//!
//! Entry points: [`parse_strict`] (`toLiquidHtmlAST`), [`parse_tolerant`]
//! (`toTolerantLiquidHtmlAST`), [`parse_liquid_only`] (`toLiquidAST`) and
//! [`parse_tolerant_liquid_only`] (`toTolerantLiquidAST`). Positions are UTF-8
//! byte offsets; [`SourceMap`] converts to UTF-16 and line/character.
//!
//! The parser recurses on nesting depth, so run it on a thread with a large stack.

pub mod ast;
mod conditional_comment;
pub mod document;
pub mod dump;
mod errors;
pub mod grammar;
pub mod js;
mod lax_recover;
mod liquid_doc;
pub mod markup;
mod source_map;
pub mod tags;
mod tolerant;

pub use ast::*;
pub use conditional_comment::{get_conditional_comment, ConditionalComment};
pub use document::tokenizer::{tokenize, Token, TokenType, TokenizeOptions};
pub use errors::{Failure, LineCol, Loc, PResult, ParseError, UnclosedNode, PARSING_ERROR_NAME};
pub use grammar::{BLOCKS, HTML_RAW_TAGS, RAW_TAGS, TAGS_WITHOUT_MARKUP, VOID_ELEMENTS};
pub use lax_recover::{lax_recover_tag_markup, lax_recover_variable, lax_recover_when_values};
pub use markup::parser::{BodyContext, MarkupParser};
pub use markup::tokenizer::{tokenize_markup, MarkupToken, MarkupTokenType};
pub use source_map::{LineCharacter, SourceMap};
pub use tolerant::{find_error_node_at_offset, ErrorNodeAtOffset};

/// Upstream's `ASTBuildOptions` plus the choice of parser class and `parseHtml`.
#[derive(Clone, Copy, Debug)]
pub struct ParseOptions {
    pub parse_html: bool,
    pub allow_unclosed_document_node: bool,
    /// `TolerantDocumentParser`: recover into `LiquidErrorNode`s.
    pub tolerant: bool,
}

/// Parses with explicit options. Prefer the named entry points, which use
/// upstream's default options.
pub fn parse(source: &str, options: ParseOptions) -> Result<Document<'_>, ParseError> {
    let tokens = tokenize(source, TokenizeOptions::default());
    let mut parser = document::parser::DocumentParser::new(
        tokens,
        source,
        options.parse_html,
        options.allow_unclosed_document_node,
        options.tolerant,
    );
    match parser.parse_document() {
        Ok(mut root) => {
            let node_count = assign_node_ids(&mut root, 0);
            Ok(Document { source, root, node_count })
        }
        Err(e) => Err(e.into_parse_error(source)),
    }
}

/// `toLiquidHtmlAST(source)`: the lint path.
pub fn parse_strict(source: &str) -> Result<Document<'_>, ParseError> {
    parse(
        source,
        ParseOptions { parse_html: true, allow_unclosed_document_node: false, tolerant: false },
    )
}

/// `toTolerantLiquidHtmlAST(source)`: the formatter path. Parsing failures become
/// `LiquidErrorNode`s; it can still fail as upstream can throw, on tags named
/// after `Object.prototype` members such as `{% toString %}`.
pub fn parse_tolerant(source: &str) -> Result<Document<'_>, ParseError> {
    parse(
        source,
        ParseOptions { parse_html: true, allow_unclosed_document_node: true, tolerant: true },
    )
}

/// `toLiquidAST(source)`: `parseHtml: false`.
pub fn parse_liquid_only(source: &str) -> Result<Document<'_>, ParseError> {
    parse(
        source,
        ParseOptions { parse_html: false, allow_unclosed_document_node: true, tolerant: false },
    )
}

/// `toTolerantLiquidAST(source)`.
pub fn parse_tolerant_liquid_only(source: &str) -> Result<Document<'_>, ParseError> {
    parse(
        source,
        ParseOptions { parse_html: false, allow_unclosed_document_node: true, tolerant: true },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_ids<'a>(node: &'a Node<'a>, out: &mut Vec<u32>) {
        out.push(node.id().0);
        if let Node::NamedArgument(a) = node {
            collect_ids(&a.value, out);
        } else {
            node.for_each_child(|c| collect_ids(c, out));
        }
    }

    #[test]
    fn ids_are_dense_and_include_block_array_literals() {
        let source = "<a b=\"{{ c | d: e }}\">{% block 'f', g: [1, h] %}{% if i %}j{% endif %}{% endblock %}</a>";
        let doc = parse_strict(source).unwrap();
        let mut ids = Vec::new();
        collect_ids(&doc.root, &mut ids);
        ids.sort_unstable();
        assert_eq!(ids, (0..doc.node_count).collect::<Vec<_>>());
    }

    #[test]
    fn walk_is_post_order_and_skips_block_array_literals() {
        let doc = parse_strict("{% block 'a', b: [1] %}{% endblock %}").unwrap();
        let mut types = Vec::new();
        walk(&doc.root, &mut |n: &Node<'_>, _| types.push(n.node_type()));
        assert_eq!(
            types,
            ["String", "NamedArgument", "BlockMarkup", "LiquidTag", "Document"]
        );
    }

    #[test]
    fn strict_error_has_upstream_shape() {
        let e = parse_strict("\u{1F600}\n<a>").unwrap_err();
        assert_eq!(e.name, "LiquidHTMLParsingError");
        assert_eq!(e.message, "Attempting to end parsing before HtmlElement 'a' was closed");
        let loc = e.loc.unwrap();
        assert_eq!(loc.start, LineCol { line: 2, column: 1 });
        // The end index clamps to the last UTF-16 unit.
        assert_eq!(loc.end, LineCol { line: 2, column: 3 });
    }

    #[test]
    fn empty_source_location_falls_back_to_one_one() {
        let e = parse_strict("{% if %}").unwrap_err();
        assert_eq!(e.loc.unwrap().start.line, 1);
        assert!(parse_strict("").is_ok());
    }

    #[test]
    fn object_prototype_names_behave_as_in_js() {
        let e = parse_tolerant("{% toString %}").unwrap_err();
        assert_eq!((e.name.as_str(), e.loc), ("Error", None));
        assert_eq!(e.message, "Unexpected object: undefined");
        let doc = parse_strict("{{ toString }}").unwrap();
        let Node::LiquidVariableOutput(output) = &doc.children()[0] else { panic!() };
        let Markup::Node(variable) = &output.markup else { panic!() };
        let Node::LiquidVariable(variable) = &**variable else { panic!() };
        assert!(matches!(&variable.expression, Node::LiquidLiteral(l)
            if l.value == LiteralValue::ObjectPrototypeFunction));
    }

    #[test]
    fn tolerant_recovers_into_error_nodes() {
        let doc = parse_tolerant("{% endfor %}{{ a }}{% endif %}").unwrap();
        let types: Vec<_> = doc.children().iter().map(Node::node_type).collect();
        assert_eq!(types, ["LiquidErrorNode", "LiquidVariableOutput", "LiquidErrorNode"]);
        let found = find_error_node_at_offset(&doc.root, 2).unwrap();
        assert_eq!(found.node.position(), Position::new(0, 12));
        assert_eq!(found.ancestors.len(), 1);
        assert!(find_error_node_at_offset(&doc.root, 13).is_none());
    }

    #[test]
    fn conditional_comment() {
        let c = get_conditional_comment("<!--[if IE]> a <![endif]-->").unwrap();
        assert_eq!((c.start_tag, c.body, c.end_tag), ("<!--[if IE]>", "a", "<![endif]-->"));
        assert!(get_conditional_comment("<!-- a -->").is_none());
    }

    #[test]
    fn lax_recovery() {
        let n = lax_recover_variable("123foo", "123foo", 0).unwrap();
        let Node::LiquidVariable(v) = &n else { panic!() };
        assert!(matches!(&v.expression, Node::VariableLookup(l) if l.name == Some("123foo")));
        assert!(lax_recover_tag_markup("increment", " ", " ", 0, 1).unwrap().is_none());
        let e = lax_recover_tag_markup("nope", "", "", 0, 0).unwrap_err();
        assert_eq!(e.name, "LaxTagRecoveryError");
    }
}
