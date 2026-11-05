//! Child list post-processing: text merging, trimming, close-tag name matching.

use crate::ast::*;
use crate::js;
use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChildFilterMode {
    /// Merge adjacent text, trim whitespace at boundaries, drop empty.
    Syntactic,
    /// Merge adjacent text, preserving all whitespace.
    Preserve,
    /// Merge, then strip whitespace-only text from the first and last positions.
    StripEdges,
}

pub(crate) fn filter_children<'s>(
    mode: ChildFilterMode,
    children: Vec<Node<'s>>,
    source: &'s str,
) -> Vec<Node<'s>> {
    match mode {
        ChildFilterMode::Syntactic => merge_adjacent_text_nodes_trimmed(children, source),
        ChildFilterMode::Preserve => merge_adjacent_text_nodes(children, source),
        ChildFilterMode::StripEdges => merge_adjacent_text_nodes_strip_edges(children, source),
    }
}

pub(crate) fn merge_adjacent_text_nodes<'s>(
    nodes: Vec<Node<'s>>,
    source: &'s str,
) -> Vec<Node<'s>> {
    if nodes.len() <= 1 {
        return nodes;
    }
    let mut result: Vec<Node<'s>> = Vec::with_capacity(nodes.len());
    for node in nodes {
        if let Node::TextNode(text) = &node {
            if let Some(Node::TextNode(prev)) = result.last_mut() {
                prev.position.end = text.position.end;
                prev.value =
                    Cow::Borrowed(js::slice(source, prev.position.start, prev.position.end));
                continue;
            }
        }
        result.push(node);
    }
    result
}

fn merge_adjacent_text_nodes_trimmed<'s>(nodes: Vec<Node<'s>>, source: &'s str) -> Vec<Node<'s>> {
    let mut merged = merge_adjacent_text_nodes(nodes, source);
    // After merging, no two neighbours are both text, so each text node has
    // non-text (or nothing) on both sides and is trimmed on both.
    merged.retain_mut(|node| {
        let Node::TextNode(text) = node else { return true };
        let (lead, trail, empty) = {
            let value: &str = &text.value;
            let after_start = js::trim_start(value);
            let after_both = js::trim_end(after_start);
            (
                value.len() - after_start.len(),
                after_start.len() - after_both.len(),
                after_both.is_empty(),
            )
        };
        if empty {
            return false;
        }
        if lead > 0 || trail > 0 {
            text.position.start += lead as u32;
            text.position.end -= trail as u32;
            text.value = match &text.value {
                Cow::Borrowed(s) => Cow::Borrowed(&s[lead..s.len() - trail]),
                Cow::Owned(s) => Cow::Owned(s[lead..s.len() - trail].to_string()),
            };
        }
        true
    });
    merged
}

fn is_whitespace_only_text_node(node: &Node<'_>) -> bool {
    matches!(node, Node::TextNode(t) if js::is_all_ws_nonempty(&t.value))
}

fn merge_adjacent_text_nodes_strip_edges<'s>(
    nodes: Vec<Node<'s>>,
    source: &'s str,
) -> Vec<Node<'s>> {
    let mut merged = merge_adjacent_text_nodes(nodes, source);
    let mut end = merged.len();
    let mut start = 0;
    while start < end && is_whitespace_only_text_node(&merged[start]) {
        start += 1;
    }
    while end > start && is_whitespace_only_text_node(&merged[end - 1]) {
        end -= 1;
    }
    merged.truncate(end);
    merged.drain(..start);
    merged
}

fn is_wholly_liquid_tag_name(segments: &[Node<'_>]) -> bool {
    segments.len() == 1 && matches!(segments[0], Node::LiquidTag(_) | Node::LiquidRawTag(_))
}

fn variable_output_markup<'a>(output: &'a LiquidVariableOutput<'_>) -> &'a str {
    match &output.markup {
        Markup::Str(s) => s,
        Markup::Node(n) => match &**n {
            Node::LiquidVariable(v) => v.raw_source,
            _ => "",
        },
        _ => "",
    }
}

pub(crate) fn compound_names_match(a: &[Node<'_>], b: &[Node<'_>], source: &str) -> bool {
    if is_wholly_liquid_tag_name(a) && is_wholly_liquid_tag_name(b) {
        return true;
    }
    if a.len() != b.len() {
        return false;
    }
    for (node_a, node_b) in a.iter().zip(b) {
        if std::mem::discriminant(node_a) != std::mem::discriminant(node_b) {
            return false;
        }
        match (node_a, node_b) {
            (Node::TextNode(x), Node::TextNode(y)) => {
                if x.value != y.value {
                    return false;
                }
            }
            (Node::LiquidVariableOutput(x), Node::LiquidVariableOutput(y)) => {
                if variable_output_markup(x) != variable_output_markup(y) {
                    return false;
                }
            }
            _ => {
                let (pa, pb) = (node_a.position(), node_b.position());
                if js::slice(source, pa.start, pa.end) != js::slice(source, pb.start, pb.end) {
                    return false;
                }
            }
        }
    }
    true
}
