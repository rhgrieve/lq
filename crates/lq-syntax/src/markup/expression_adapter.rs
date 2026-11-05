//! Parser-internal binary expressions and their conversion to AST nodes.

use crate::ast::*;

pub(crate) enum Expr<'s> {
    Value(Node<'s>),
    Comparison {
        left: Node<'s>,
        op: &'s str,
        right: Node<'s>,
        position: Position,
    },
    Logical {
        left: Box<Expr<'s>>,
        relation: LogicalRelation,
        right: Box<Expr<'s>>,
        /// Start of the `and`/`or`; a nested logical right child's position
        /// starts here.
        op_start: u32,
        position: Position,
    },
}

impl Expr<'_> {
    pub(crate) fn position(&self) -> Position {
        match self {
            Expr::Value(n) => n.position(),
            Expr::Comparison { position, .. } | Expr::Logical { position, .. } => *position,
        }
    }
}

pub(crate) fn adapt_conditional(expr: Expr<'_>) -> Node<'_> {
    match expr {
        Expr::Value(n) => n,
        Expr::Logical { left, relation, right, op_start, position } => {
            let mut right = adapt_conditional(*right);
            if let Node::LogicalExpression(r) = &mut right {
                r.position.start = op_start;
            }
            Node::LogicalExpression(Box::new(LogicalExpression {
                id: NodeId(0),
                position,
                relation,
                left: adapt_conditional(*left),
                right,
            }))
        }
        Expr::Comparison { left, op, right, position } => {
            Node::Comparison(Box::new(Comparison {
                id: NodeId(0),
                position,
                comparator: op,
                left,
                right,
            }))
        }
    }
}

pub(crate) fn adapt_complex(expr: Expr<'_>) -> Node<'_> {
    match expr {
        Expr::Value(n) => n,
        other => {
            let position = other.position();
            Node::BooleanExpression(Box::new(BooleanExpression {
                id: NodeId(0),
                position,
                condition: adapt_conditional(other),
            }))
        }
    }
}
