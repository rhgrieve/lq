//! Lax recovery of malformed markup, for callers reproducing Ruby's rendered
//! output. Nothing here is reachable from the document parsers. Returned nodes
//! carry ids numbered from 0.

use crate::ast::*;
use crate::errors::{Failure, ParseError, PResult};
use crate::js;
use crate::markup::parser::MarkupParser;
use crate::markup::tokenizer::tokenize_markup;
use crate::tags::{self, LiquidLineContext, StatementParser, TagDefinition};

struct NoopParser;

impl<'s> StatementParser<'s> for NoopParser {
    fn parse_liquid_statement(
        &mut self,
        _tag_name: &'s str,
        _markup_string: &'s str,
        _markup_offset: u32,
        _ctx: &mut LiquidLineContext<'s>,
    ) -> PResult<Node<'s>> {
        Err(Failure::error("lax-recover: nested statement parsing is not supported during recovery"))
    }
}

fn number_markup(markup: &mut Markup<'_>) {
    let mut next = 0;
    match markup {
        Markup::Node(n) => {
            assign_node_ids(n, next);
        }
        Markup::List(l) => {
            for n in l {
                next = assign_node_ids(n, next);
            }
        }
        Markup::Str(_) | Markup::None => {}
    }
}

/// `laxRecoverVariable`. `None` when even lax parsing cannot produce a value.
pub fn lax_recover_variable<'s>(
    raw_markup: &'s str,
    source: &'s str,
    offset: u32,
) -> Option<Node<'s>> {
    let tokens = tokenize_markup(raw_markup, offset);
    let mut parser =
        MarkupParser::new(tokens, source, Some(offset), Some(offset + raw_markup.len() as u32));
    parser.enable_lax();
    let mut variable = parser.liquid_variable().ok()?;
    assign_node_ids(&mut variable, 0);
    Some(variable)
}

/// `laxRecoverTagMarkup`. `Ok(None)` is upstream's `undefined`: empty
/// `increment`/`decrement` markup. An unknown tag fails with the name
/// `LaxTagRecoveryError`; malformed markup fails with the parse error.
pub fn lax_recover_tag_markup<'s>(
    tag_name: &str,
    markup_string: &'s str,
    source: &'s str,
    offset: u32,
    markup_end: u32,
) -> Result<Option<Markup<'s>>, ParseError> {
    let id = match tags::tag_for_name(tag_name) {
        Some(TagDefinition::Tag { id }) | Some(TagDefinition::Block { id, .. }) => id,
        _ => {
            return Err(ParseError {
                name: "LaxTagRecoveryError".to_string(),
                message: format!("Unknown tag '{tag_name}'"),
                loc: None,
                unclosed: None,
            })
        }
    };
    if (tag_name == "increment" || tag_name == "decrement") && js::trim(markup_string).is_empty() {
        return Ok(None);
    }
    let tokens = tokenize_markup(markup_string, offset);
    let mut parser = MarkupParser::new(tokens, source, Some(offset), Some(markup_end));
    parser.enable_lax();
    let mut markup = tags::parse_tag(id, tag_name, &mut parser, &mut NoopParser)
        .map_err(|e| e.into_parse_error(source))?;
    number_markup(&mut markup);
    Ok(Some(markup))
}

/// `laxRecoverWhenValues`.
pub fn lax_recover_when_values<'s>(
    markup_string: &'s str,
    source: &'s str,
    offset: u32,
    markup_end: u32,
) -> Option<Vec<Node<'s>>> {
    let tokens = tokenize_markup(markup_string, offset);
    let mut parser = MarkupParser::new(tokens, source, Some(offset), Some(markup_end));
    parser.enable_lax();
    let mut values = tags::when_branch_parse(&mut parser).ok()?;
    let mut next = 0;
    for v in &mut values {
        next = assign_node_ids(v, next);
    }
    Some(values)
}
