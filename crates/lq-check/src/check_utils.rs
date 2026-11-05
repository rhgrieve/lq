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

/// A name as hints compare it: ignoring case, and with `-` the same as `_`.
fn fold_name(name: &str) -> Vec<char> {
    name.chars().map(|c| if c == '-' { '_' } else { c.to_ascii_lowercase() }).collect()
}

/// The optimal string alignment distance, or `None` once it must be over `limit`.
fn bounded_distance(a: &[char], b: &[char], limit: usize) -> Option<usize> {
    let mut before = vec![0; b.len() + 1];
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut row = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        row[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            row[j] = (previous[j] + 1).min(row[j - 1] + 1).min(previous[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                row[j] = row[j].min(before[j - 2] + 1);
            }
        }
        // A transposition reaches back two rows, so both have to be over the limit.
        if row.iter().min() > Some(&limit) && previous.iter().min() > Some(&limit) {
            return None;
        }
        std::mem::swap(&mut before, &mut previous);
        std::mem::swap(&mut previous, &mut row);
    }
    let distance = previous[b.len()];
    (distance <= limit).then_some(distance)
}

/// The candidate `name` was most likely meant to be, or `None` if none is close: a wrong guess
/// is worse than no guess. For a report's hints only, never for deciding what to report.
pub fn closest<'c>(name: &str, candidates: impl IntoIterator<Item = &'c str>) -> Option<&'c str> {
    let folded = fold_name(name);
    if folded.len() < 3 {
        return None;
    }
    let limit = (folded.len() / 4).clamp(1, 3);
    candidates
        .into_iter()
        .filter(|c| *c != name && c.chars().count().abs_diff(folded.len()) <= limit)
        .filter_map(|c| bounded_distance(&folded, &fold_name(c), limit).map(|d| (d, c)))
        .min()
        .map(|(_, c)| c)
}

pub fn did_you_mean(candidate: &str) -> String {
    format!("did you mean '{candidate}'?")
}

#[cfg(test)]
mod closest_tests {
    use super::closest;

    #[test]
    fn finds_near_names_only() {
        let names = ["product-card", "image", "button", "price_list"];
        assert_eq!(closest("product-crad", names), Some("product-card"));
        assert_eq!(closest("product_card", names), Some("product-card"));
        assert_eq!(closest("imag", names), Some("image"));
        assert_eq!(closest("Button", names), Some("button"));
        assert_eq!(closest("vite-tag", names), None);
        assert_eq!(closest("xy", ["xz"]), None);
    }

    #[test]
    fn a_transposition_counts_as_one_edit() {
        assert_eq!(closest("abcd", ["abdc"]), Some("abdc"));
        assert_eq!(closest("subtitel", ["subtitle"]), Some("subtitle"));
    }

    #[test]
    fn never_suggests_the_name_itself() {
        assert_eq!(closest("image", ["image"]), None);
    }

    #[test]
    fn a_tie_is_decided_the_same_way_every_time() {
        assert_eq!(closest("cart", ["cars", "carp"]), Some("carp"));
        assert_eq!(closest("cart", ["carp", "cars"]), Some("carp"));
    }
}
