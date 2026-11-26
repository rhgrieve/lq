//! Liquid + HTML parser, ported from @shopify/liquid-html-parser 2.11.0, with the
//! parser changes on theme-tools `main` behind [`ParseTarget::Main`].
//!
//! Entry points: [`parse_strict`] (`toLiquidHtmlAST`), [`parse_tolerant`]
//! (`toTolerantLiquidHtmlAST`), [`parse_liquid_only`] (`toLiquidAST`) and
//! [`parse_tolerant_liquid_only`] (`toTolerantLiquidAST`) parse as 2.11.0 does;
//! the `_for` variants take a [`ParseTarget`]. Positions are UTF-8 byte offsets;
//! [`SourceMap`] converts to UTF-16 and line/character.
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

/// Which upstream parser to reproduce. Prettier plugin 1.11.3 runs on
/// liquid-html-parser 2.11.0, so `lq fmt` parses as `Released`; theme-check
/// follows theme-tools `main`, so `lq check` parses as `Main`. Every behaviour
/// change ported from `main` is conditional on `Main`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParseTarget {
    /// @shopify/liquid-html-parser 2.11.0.
    #[default]
    Released,
    /// theme-tools `main` (af2e1a78), with upstream's default `ASTBuildOptions`
    /// (no `pairDanglingCloseTags`, no `opaqueHtmlComments`).
    Main,
}

impl ParseTarget {
    #[inline]
    pub fn is_main(self) -> bool {
        self == ParseTarget::Main
    }
}

/// Upstream's `ASTBuildOptions` plus the choice of parser class and `parseHtml`.
#[derive(Clone, Copy, Debug)]
pub struct ParseOptions {
    pub parse_html: bool,
    pub allow_unclosed_document_node: bool,
    /// `TolerantDocumentParser`: recover into `LiquidErrorNode`s.
    pub tolerant: bool,
    pub target: ParseTarget,
}

/// Parses with explicit options. Prefer the named entry points, which use
/// upstream's default options.
pub fn parse(source: &str, options: ParseOptions) -> Result<Document<'_>, ParseError> {
    // `tokenize(source, { rawTags: env, builtinRawTags: true, parseHtml })` on main.
    let (tokens, raw_body_ends) = document::tokenizer::tokenize_document(
        source,
        TokenizeOptions {
            target: options.target,
            raw_tags: true,
            builtin_raw_tags: true,
            liquid_only: !options.parse_html,
            ..TokenizeOptions::default()
        },
    );
    let mut parser = document::parser::DocumentParser::new(
        tokens,
        source,
        options.parse_html,
        options.allow_unclosed_document_node,
        options.tolerant,
        options.target,
    );
    parser.raw_body_ends = raw_body_ends;
    match parser.parse_document() {
        Ok(mut root) => {
            let node_count = assign_node_ids(&mut root, 0);
            Ok(Document { source, root, node_count, target: options.target })
        }
        Err(e) => Err(e.into_parse_error(source)),
    }
}

/// `toLiquidHtmlAST(source)` as 2.11.0 parses it.
pub fn parse_strict(source: &str) -> Result<Document<'_>, ParseError> {
    parse_strict_for(source, ParseTarget::Released)
}

/// `toLiquidHtmlAST(source)`: the lint path.
pub fn parse_strict_for(source: &str, target: ParseTarget) -> Result<Document<'_>, ParseError> {
    parse(
        source,
        ParseOptions {
            parse_html: true,
            allow_unclosed_document_node: false,
            tolerant: false,
            target,
        },
    )
}

/// `toTolerantLiquidHtmlAST(source)` as 2.11.0 parses it: the formatter path.
/// Parsing failures become `LiquidErrorNode`s; it can still fail as upstream can
/// throw, on tags named after `Object.prototype` members such as `{% toString %}`.
pub fn parse_tolerant(source: &str) -> Result<Document<'_>, ParseError> {
    parse_tolerant_for(source, ParseTarget::Released)
}

/// `toTolerantLiquidHtmlAST(source)`.
pub fn parse_tolerant_for(source: &str, target: ParseTarget) -> Result<Document<'_>, ParseError> {
    parse(
        source,
        ParseOptions { parse_html: true, allow_unclosed_document_node: true, tolerant: true, target },
    )
}

/// `toLiquidAST(source)` as 2.11.0 parses it: `parseHtml: false`.
pub fn parse_liquid_only(source: &str) -> Result<Document<'_>, ParseError> {
    parse_liquid_only_for(source, ParseTarget::Released)
}

/// `toLiquidAST(source)`.
pub fn parse_liquid_only_for(source: &str, target: ParseTarget) -> Result<Document<'_>, ParseError> {
    parse(
        source,
        ParseOptions {
            parse_html: false,
            allow_unclosed_document_node: true,
            tolerant: false,
            target,
        },
    )
}

/// `toTolerantLiquidAST(source)` as 2.11.0 parses it.
pub fn parse_tolerant_liquid_only(source: &str) -> Result<Document<'_>, ParseError> {
    parse_tolerant_liquid_only_for(source, ParseTarget::Released)
}

/// `toTolerantLiquidAST(source)`.
pub fn parse_tolerant_liquid_only_for(
    source: &str,
    target: ParseTarget,
) -> Result<Document<'_>, ParseError> {
    parse(
        source,
        ParseOptions { parse_html: false, allow_unclosed_document_node: true, tolerant: true, target },
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
    fn main_parses_the_liquid_inside_html_comments() {
        let source = "<!-- {{ a }} <b -->";
        let doc = parse_strict_for(source, ParseTarget::Main).unwrap();
        let Node::HtmlComment(c) = &doc.children()[0] else { panic!() };
        assert_eq!(c.body, "{{ a }} <b");
        let types: Vec<_> = c.children.iter().map(Node::node_type).collect();
        assert_eq!(types, ["LiquidVariableOutput", "TextNode"]);
        assert_eq!((c.block_start_position, c.block_end_position), (Position::new(0, 4), Position::new(16, 19)));
        let doc = parse_strict(source).unwrap();
        let Node::HtmlComment(c) = &doc.children()[0] else { panic!() };
        assert!(c.children.is_empty());
    }

    #[test]
    fn main_reads_script_bodies_as_raw_text() {
        let tokens = |target| -> Vec<TokenType> {
            tokenize("<script>i<n</script>", TokenizeOptions::for_target(target))
                .iter()
                .map(|t| t.token_type)
                .collect()
        };
        assert_eq!(tokens(ParseTarget::Released).iter().filter(|&&t| t == TokenType::HtmlTagOpen).count(), 2);
        assert_eq!(tokens(ParseTarget::Main).iter().filter(|&&t| t == TokenType::HtmlTagOpen).count(), 1);
    }

    #[test]
    fn only_main_names_the_unclosed_node() {
        let e = parse_strict_for("<h{{ a }} b>", ParseTarget::Main).unwrap_err();
        let u = e.unclosed.unwrap();
        assert_eq!((u.node_type, u.name.as_str()), ("HtmlElement", "h{{ a }}"));
        assert_eq!(u.block_start_position, Position::new(0, 12));
        assert!(parse_strict("<h{{ a }} b>").unwrap_err().unclosed.is_none());
    }

    #[test]
    fn main_parses_object_prototype_names_as_ordinary_names() {
        let doc = parse_tolerant_for("{% toString %}{{ constructor }}", ParseTarget::Main).unwrap();
        let types: Vec<_> = doc.children().iter().map(Node::node_type).collect();
        assert_eq!(types, ["LiquidTag", "LiquidVariableOutput"]);
        let Node::LiquidVariableOutput(output) = &doc.children()[1] else { panic!() };
        let Markup::Node(variable) = &output.markup else { panic!() };
        let Node::LiquidVariable(variable) = &**variable else { panic!() };
        assert!(matches!(&variable.expression, Node::VariableLookup(l) if l.name == Some("constructor")));
    }

    #[test]
    fn a_filter_starts_at_the_whitespace_before_its_pipe_as_upstream() {
        // Both upstream parsers start the filter at the U+00A0 before `|`. A
        // check that reports from the `|` steps over that char, not one byte.
        for target in [ParseTarget::Released, ParseTarget::Main] {
            let source = "{{ 'foo'\u{A0}| bar }}";
            let doc = parse_strict_for(source, target).unwrap();
            let Node::LiquidVariableOutput(output) = &doc.children()[0] else { panic!() };
            let Markup::Node(variable) = &output.markup else { panic!() };
            let Node::LiquidVariable(variable) = &**variable else { panic!() };
            let filter = variable.filters[0].position();
            assert_eq!(&source[filter.start as usize..filter.end as usize], "\u{A0}| bar");
        }
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
