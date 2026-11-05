//! `checks/utils.ts`: helpers shared by checks.

use lq_json::js::is_js_whitespace;
use lq_syntax::{AttrNode, Markup, Node};

/// `getHtmlNodeName(node)`: the static name of an element, `None` when dynamic.
pub fn html_node_name<'n>(node: &'n Node<'_>) -> Option<&'n str> {
    match node {
        Node::HtmlVoidElement(n) => Some(n.name),
        Node::HtmlRawNode(n) => Some(n.name),
        Node::HtmlElement(n) => single_text(&n.name),
        Node::HtmlSelfClosingElement(n) => single_text(&n.name),
        _ => None,
    }
}

fn single_text<'n>(name: &'n [Node<'_>]) -> Option<&'n str> {
    match name {
        [Node::TextNode(t)] => Some(&t.value),
        _ => None,
    }
}

/// `isHtmlTag(node, name)`.
pub fn is_html_tag(node: &Node<'_>, name: &str) -> bool {
    matches!(node, Node::HtmlElement(n) if single_text(&n.name) == Some(name))
}

/// The attributes of an HTML node.
pub fn html_attributes<'n, 's>(node: &'n Node<'s>) -> &'n [Node<'s>] {
    match node {
        Node::HtmlElement(n) => &n.attributes,
        Node::HtmlVoidElement(n) => &n.attributes,
        Node::HtmlSelfClosingElement(n) => &n.attributes,
        Node::HtmlRawNode(n) => &n.attributes,
        _ => &[],
    }
}

/// `isValuedHtmlAttribute(attr)`, returning the attribute.
pub fn valued_attribute<'n, 's>(attr: &'n Node<'s>) -> Option<&'n AttrNode<'s>> {
    match attr {
        Node::AttrSingleQuoted(a) | Node::AttrDoubleQuoted(a) | Node::AttrUnquoted(a) => Some(a),
        _ => None,
    }
}

/// `isHtmlAttribute(attr)`.
pub fn is_html_attribute(attr: &Node<'_>) -> bool {
    matches!(
        attr,
        Node::AttrSingleQuoted(_) | Node::AttrDoubleQuoted(_) | Node::AttrUnquoted(_) | Node::AttrEmpty(_)
    )
}

/// `isAttr(attr, name)`.
pub fn is_attr(attr: &Node<'_>, name: &str) -> bool {
    let parts = match attr {
        Node::AttrSingleQuoted(a) | Node::AttrDoubleQuoted(a) | Node::AttrUnquoted(a) => &a.name,
        Node::AttrEmpty(a) => &a.name,
        _ => return false,
    };
    single_text(parts) == Some(name)
}

/// `valueIncludes(attr, word)`: `word` is a whitespace-separated word of a text part.
/// `word` must not contain regex metacharacters that upstream would interpret.
pub fn value_includes(attr: &AttrNode<'_>, word: &str) -> bool {
    attr.value.iter().any(|v| match v {
        Node::TextNode(t) => t.value.split(is_js_whitespace).any(|w| w == word),
        _ => false,
    })
}

/// `hasAttributeValueOf(attr, value)`.
pub fn has_attribute_value_of(attr: &AttrNode<'_>, value: &str) -> bool {
    single_text(&attr.value) == Some(value)
}

/// `isLoopLiquidTag(tag)`.
pub fn is_loop_tag(name: &str) -> bool {
    matches!(name, "for" | "tablerow")
}

/// `isLoopScopedVariable(name, ancestors)`.
pub fn is_loop_scoped_variable(name: &str, ancestors: &[&Node<'_>]) -> bool {
    ancestors.iter().any(|a| match a {
        Node::LiquidTag(t) if is_loop_tag(t.name) => match &t.markup {
            Markup::Node(m) => matches!(&**m, Node::ForMarkup(f) if f.variable_name == name),
            _ => false,
        },
        _ => false,
    })
}

/// `isWithinRawTagThatDoesNotParseItsContents(ancestors)`.
#[inline]
pub fn is_within_unparsed_raw_tag(ancestors: &[&Node<'_>]) -> bool {
    ancestors.iter().any(|a| {
        matches!(a, Node::LiquidRawTag(t) if matches!(t.name, "raw" | "stylesheet" | "javascript" | "schema"))
    })
}

/// `isLiquidTagAssign`/`isLiquidTagCapture`-style: the tag's parsed markup node.
#[inline]
pub fn tag_markup<'n, 's>(markup: &'n Markup<'s>) -> Option<&'n Node<'s>> {
    match markup {
        Markup::Node(n) => Some(n),
        _ => None,
    }
}

/// `isContentForBlock(markup)`.
pub fn is_content_for_block(markup: &Markup<'_>) -> bool {
    match markup {
        Markup::Node(m) => match &**m {
            Node::ContentForMarkup(c) => matches!(&c.content_for_type, Node::String(s) if s.value == "block"),
            _ => false,
        },
        _ => false,
    }
}

/// `tags/content-for.ts`.
pub const RESERVED_CONTENT_FOR_ARGUMENTS: [&str; 19] = [
    "attributes", "block", "blocks", "class", "context", "inherit", "resource", "resources", "schema",
    "section", "sections", "settings", "snippet", "snippets", "src", "style", "styles", "template", "templates",
];
pub const REQUIRED_CONTENT_FOR_ARGUMENTS: [&str; 2] = ["id", "type"];
pub const CLOSEST_ARGUMENT: &str = "closest.";

/// `node.source.slice(...)` of a node's text in its file.
pub fn node_text<'s>(source: &'s str, node: &Node<'_>) -> &'s str {
    let p = node.position();
    lq_syntax::js::slice(source, p.start, p.end)
}
