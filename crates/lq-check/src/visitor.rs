//! AST traversal: `visitLiquid`, `visitJSON`, `visit` and `findCurrentNode`.
//!
//! Upstream walks with a stack, pushing each own property of a node in key order
//! (arrays pushed in reverse). So a node's fields are visited in REVERSE key order,
//! while the items of an array field stay in order, and `X:exit` happens right
//! after `X`, before any child.

use lq_json::JsonNode;
use lq_syntax::{Markup, Node};

/// Calls `f` with each child in upstream's visiting order.
#[inline]
pub fn for_each_visit_child<'a, F: FnMut(&'a Node<'a>)>(node: &'a Node<'a>, mut f: F) {
    macro_rules! list {
        ($l:expr) => {
            for c in $l.iter() {
                f(c);
            }
        };
    }
    macro_rules! markup {
        ($m:expr) => {
            match $m {
                Markup::Node(c) => f(c),
                Markup::List(l) => list!(l),
                Markup::Str(_) | Markup::None => {}
            }
        };
    }
    match node {
        Node::Document(n) => list!(n.children),
        Node::LiquidRawTag(n) => f(&n.body),
        Node::LiquidTag(n) => {
            if let Some(children) = &n.children {
                list!(children);
            }
            markup!(&n.markup);
        }
        Node::LiquidBranch(n) => {
            list!(n.children);
            markup!(&n.markup);
        }
        Node::LiquidVariableOutput(n) => markup!(&n.markup),
        Node::HtmlSelfClosingElement(n) => {
            list!(n.attributes);
            list!(n.name);
        }
        Node::HtmlVoidElement(n) => list!(n.attributes),
        Node::HtmlElement(n) => {
            list!(n.children);
            list!(n.attributes);
            list!(n.name);
        }
        Node::HtmlDanglingMarkerClose(n) => list!(n.name),
        Node::HtmlRawNode(n) => {
            f(&n.body);
            list!(n.attributes);
        }
        Node::AttrSingleQuoted(n) | Node::AttrDoubleQuoted(n) | Node::AttrUnquoted(n) => {
            list!(n.value);
            list!(n.name);
        }
        Node::AttrEmpty(n) => list!(n.name),
        Node::LiquidVariable(n) => {
            list!(n.filters);
            f(&n.expression);
        }
        Node::LiquidFilter(n) => list!(n.args),
        Node::NamedArgument(n) => f(&n.value),
        Node::BooleanExpression(n) => f(&n.condition),
        Node::Range(n) => {
            f(&n.end);
            f(&n.start);
        }
        Node::VariableLookup(n) => list!(n.lookups),
        Node::Comparison(n) => {
            f(&n.right);
            f(&n.left);
        }
        Node::LogicalExpression(n) => {
            f(&n.right);
            f(&n.left);
        }
        Node::AssignMarkup(n) => f(&n.value),
        Node::BlockMarkup(n) => {
            list!(n.args);
            f(&n.name);
        }
        Node::ContentForMarkup(n) => {
            list!(n.args);
            f(&n.content_for_type);
        }
        Node::CycleMarkup(n) => {
            list!(n.args);
            if let Some(c) = &n.group_name {
                f(c);
            }
        }
        Node::ForMarkup(n) => {
            list!(n.args);
            f(&n.collection);
        }
        Node::PaginateMarkup(n) => {
            list!(n.args);
            f(&n.page_size);
            f(&n.collection);
        }
        Node::RawMarkup(n) => list!(n.nodes),
        Node::RenderMarkup(n) => {
            list!(n.args);
            if let Some(c) = &n.alias {
                f(c);
            }
            if let Some(c) = &n.variable {
                f(c);
            }
            f(&n.snippet);
        }
        Node::SectionMarkup(n) => {
            list!(n.args);
            f(&n.name);
        }
        Node::RenderVariableExpression(n) => f(&n.name),
        Node::LiquidDocParamNode(n) => {
            if let Some(c) = &n.param_description {
                f(c);
            }
            if let Some(c) = &n.param_type {
                f(c);
            }
            f(&n.param_name);
        }
        Node::LiquidDocDescriptionNode(n) => f(&n.content),
        Node::LiquidDocExampleNode(n) => f(&n.content),
        Node::LiquidDocPromptNode(n) => f(&n.content),
        Node::BlockArrayLiteral(n) => list!(n.elements),
        Node::YamlFrontmatter(_)
        | Node::HtmlDoctype(_)
        | Node::HtmlComment(_)
        | Node::TextNode(_)
        | Node::LiquidLiteral(_)
        | Node::String(_)
        | Node::Number(_)
        | Node::RenderAliasExpression(_)
        | Node::LiquidErrorNode(_) => {}
    }
}

/// `visit(ast, visitor)`: pre-order in upstream's order, passing the lineage. For
/// collecting things; `f` pushes results itself.
pub fn visit<'a, F: FnMut(&'a Node<'a>, &[&'a Node<'a>])>(root: &'a Node<'a>, f: &mut F) {
    fn inner<'a, F: FnMut(&'a Node<'a>, &[&'a Node<'a>])>(
        node: &'a Node<'a>,
        lineage: &mut Vec<&'a Node<'a>>,
        f: &mut F,
    ) {
        f(node, lineage);
        lineage.push(node);
        for_each_visit_child(node, |c| inner(c, lineage, f));
        lineage.pop();
    }
    inner(root, &mut Vec::new(), f);
}

/// Calls `f` with each child of a JSON node in `visitJSON`'s order: a `Property`'s
/// value before its key, array and object children in order.
#[inline]
pub fn for_each_json_child<'a, F: FnMut(&'a JsonNode<'a>)>(node: &'a JsonNode<'a>, mut f: F) {
    match node {
        JsonNode::Object { children, .. } | JsonNode::Array { children, .. } => {
            for c in children {
                f(c);
            }
        }
        JsonNode::Property { key, value, .. } => {
            f(value);
            f(key);
        }
        JsonNode::Identifier { .. } | JsonNode::Literal { .. } => {}
    }
}

/// Children as `forEachChildNodes` calls them, which for a stack means the REVERSE
/// of visiting order: the last call wins in `findCurrentNode`.
fn for_each_child_node_call_order<'a>(node: &'a Node<'a>) -> Vec<&'a Node<'a>> {
    // Fields in key order, array items reversed. That is: visiting order reversed.
    let mut children = Vec::new();
    for_each_visit_child(node, |c| children.push(c));
    children.reverse();
    children
}

fn is_covered(node: &Node<'_>, offset: u32) -> bool {
    let p = node.position();
    match node {
        Node::String(_) | Node::VariableLookup(_) | Node::LogicalExpression(_) | Node::Comparison(_) => {
            p.start <= offset && offset <= p.end
        }
        _ => p.start < offset && offset <= p.end,
    }
}

fn size(node: &Node<'_>) -> u32 {
    let p = node.position();
    p.end.wrapping_sub(p.start)
}

/// `isUnclosed`: `blockEndPosition.end === -1`, or nonempty `children` on a node
/// without a `blockEndPosition`. Strict parsing has no `-1` ends.
fn is_unclosed(node: &Node<'_>) -> bool {
    match node {
        Node::LiquidTag(t) => {
            t.block_end_position.is_none() && t.children.as_ref().is_some_and(|c| !c.is_empty())
        }
        Node::LiquidBranch(_) | Node::HtmlElement(_) | Node::HtmlRawNode(_) | Node::LiquidRawTag(_) => false,
        Node::Document(d) => !d.children.is_empty(),
        _ => false,
    }
}

/// `findCurrentNode(ast, cursorPosition)`: `(node, ancestors)`.
pub fn find_current_node<'a>(ast: &'a Node<'a>, cursor: u32) -> (&'a Node<'a>, Vec<&'a Node<'a>>) {
    let mut current = ast;
    let mut ancestors: Vec<&'a Node<'a>> = Vec::new();
    loop {
        let prev = current;
        let base_len = ancestors.len();
        let lineage_parent = current;
        let mut next: Option<&'a Node<'a>> = None;
        for child in for_each_child_node_call_order(prev) {
            // `size(current)` sees earlier children's updates.
            let cur = next.unwrap_or(prev);
            if is_unclosed(child) || (is_covered(child, cursor) && size(child) <= size(cur)) {
                next = Some(child);
            }
        }
        match next {
            Some(n) => {
                ancestors.truncate(base_len);
                ancestors.push(lineage_parent);
                current = n;
            }
            None => return (current, ancestors),
        }
    }
}
