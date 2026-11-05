//! Helpers for the args batch of checks.

use crate::block_parameters::BLOCK_CONTENT_PARAMETER;
use crate::liquid_doc::{parse_param_type_syntax, BASIC_PARAM_TYPES};
use lq_syntax::markup::tokenizer::{tokenize_markup, MarkupTokenType};
use lq_syntax::{BlockMarkup, LiquidTag, Markup, NamedArgument, Node};
use lq_theme::docs::ThemeDocset;

/// `node.markup as BlockMarkup`, on a `{% block %}` tag that isn't a syntax error.
///
/// `name !== 'block' || blockTagSyntaxError(node)` is the shared guard of the block
/// checks. Only the truthiness of `blockTagSyntaxError` matters to them, so the
/// problem it would build is left to `LiquidSyntaxError`.
pub fn valid_block_markup<'n, 's>(tag: &'n LiquidTag<'s>, source: &str) -> Option<&'n BlockMarkup<'s>> {
    if tag.name != "block" {
        return None;
    }
    let Markup::Node(markup) = &tag.markup else { return None };
    let Node::BlockMarkup(markup) = &**markup else { return None };
    (!block_tag_has_syntax_error(tag, markup, source)).then_some(&**markup)
}

/// The `String` value of `markup.name`.
pub fn block_name<'n>(markup: &'n BlockMarkup<'_>) -> &'n str {
    match &markup.name {
        Node::String(s) => s.value,
        _ => "",
    }
}

/// The `NamedArgument` of an `args` entry.
pub fn named<'n, 's>(arg: &'n Node<'s>) -> Option<&'n NamedArgument<'s>> {
    match arg {
        Node::NamedArgument(a) => Some(a),
        _ => None,
    }
}

/// Whether `node.children?.length` is truthy.
pub fn has_children(tag: &LiquidTag<'_>) -> bool {
    tag.children.as_ref().is_some_and(|c| !c.is_empty())
}

fn block_tag_has_syntax_error(tag: &LiquidTag<'_>, markup: &BlockMarkup<'_>, source: &str) -> bool {
    let args = || markup.args.iter().filter_map(named);
    if block_name(markup).starts_with("shopify://apps/")
        && (args().any(|a| a.name != "block.name") || has_content(tag))
    {
        return true;
    }
    args().any(|a| {
        (a.name != "block.name" && a.name.contains('.'))
            || (a.name == "block.name" && !matches!(a.value, Node::String(_)))
            || matches!(&a.value, Node::VariableLookup(l) if l.name.is_none())
    }) || {
        let p = tag.markup_position;
        has_skipped_characters(lq_syntax::js::slice(source, p.start, p.end))
    }
}

fn has_content(tag: &LiquidTag<'_>) -> bool {
    tag.children.iter().flatten().any(|child| match child {
        Node::TextNode(t) => !lq_syntax::js::trim(&t.value).is_empty(),
        _ => true,
    })
}

/// `hasSkippedCharacters(markup)`: bytes outside every markup token, whitespace aside.
pub fn has_skipped_characters(markup: &str) -> bool {
    let is_skipped = |gap: &[u8]| gap.iter().any(|b| !matches!(b, b' ' | b'\n' | b'\r' | b'\t'));
    let bytes = markup.as_bytes();
    let mut pos = 0usize;
    for token in tokenize_markup(markup, 0) {
        if token.token_type == MarkupTokenType::EndOfString {
            continue;
        }
        let (start, end) = (token.start as usize, token.end as usize);
        if start > pos && is_skipped(&bytes[pos..start]) {
            return true;
        }
        pos = pos.max(end);
    }
    is_skipped(&bytes[pos.min(bytes.len())..])
}

/// `parseParamType(new Set(getValidParamTypes(entries).keys()), value) !== undefined`,
/// without building the set.
pub fn is_known_param_type(docset: &dyn ThemeDocset, value: &str) -> bool {
    parse_param_type_syntax(value).is_some_and(|(base, _)| {
        BASIC_PARAM_TYPES.contains(&base) || docset.liquid_drops().iter().any(|o| o.name == base)
    })
}

/// `argument.name === BLOCK_CONTENT_PARAMETER`.
pub fn is_content_parameter(name: &str) -> bool {
    name == BLOCK_CONTENT_PARAMETER
}
