//! `findErrorNodeAtOffset`.

use crate::ast::*;

#[derive(Clone, Debug)]
pub struct ErrorNodeAtOffset<'a, 's> {
    pub node: &'a Node<'s>,
    /// Root to parent.
    pub ancestors: Vec<&'a Node<'s>>,
}

/// The deepest `LiquidErrorNode` whose span contains byte `offset`, with its
/// ancestry. The first in `walk` order wins ties.
pub fn find_error_node_at_offset<'a, 's>(
    ast: &'a Node<'s>,
    offset: u32,
) -> Option<ErrorNodeAtOffset<'a, 's>> {
    fn visit<'a, 's>(
        node: &'a Node<'s>,
        offset: u32,
        stack: &mut Vec<&'a Node<'s>>,
        best: &mut Option<ErrorNodeAtOffset<'a, 's>>,
    ) {
        stack.push(node);
        node.for_each_child(|c| visit(c, offset, stack, best));
        stack.pop();
        if matches!(node, Node::LiquidErrorNode(_)) {
            let p = node.position();
            if offset >= p.start
                && offset <= p.end
                && best.as_ref().is_none_or(|b| stack.len() > b.ancestors.len())
            {
                *best = Some(ErrorNodeAtOffset { node, ancestors: stack.clone() });
            }
        }
    }
    let mut best = None;
    visit(ast, offset, &mut Vec::new(), &mut best);
    best
}
