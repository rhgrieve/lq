//! `AppBlockValidTags`.

use crate::api::*;
use lq_syntax::{Node, Position};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidRawTag, NodeKind::LiquidTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(AppBlockValidTags))
}

struct AppBlockValidTags;

/// `Object.values(ForbiddenTag)`.
const FORBIDDEN_TAGS: [&str; 6] = ["javascript", "stylesheet", "include", "layout", "section", "sections"];

fn report(cx: &LiquidCx<'_, '_>, name: &str, start: Position, end: u32) {
    if FORBIDDEN_TAGS.contains(&name) {
        cx.report(LiquidProblem::new(
            format!("Theme app extension blocks cannot contain '{name}' tags"),
            start.start,
            end,
        ));
    }
}

impl<'a> LiquidCheck<'a> for AppBlockValidTags {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        match node {
            Node::LiquidRawTag(tag) => report(cx, tag.name, tag.block_start_position, tag.block_end_position.end),
            Node::LiquidTag(tag) => {
                let end = tag.block_end_position.map_or(node.position().end, |p| p.end);
                report(cx, tag.name, tag.block_start_position, end);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    /// The spec's "should contain specifically the following forbidden tags", which
    /// doesn't go through `check()`.
    #[test]
    fn forbidden_tags() {
        assert_eq!(super::FORBIDDEN_TAGS, ["javascript", "stylesheet", "include", "layout", "section", "sections"]);
    }
}
