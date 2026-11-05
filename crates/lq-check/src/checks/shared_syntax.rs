//! Helpers for the syntax batch of checks: the factory bodies of `raw-tags`.

use crate::api::*;
use crate::to_schema::{is_block, is_section, is_snippet};
use lq_syntax::Node;

/// The handlers both `raw-tags` factories want.
pub const RAW_TAG_ENTER: NodeMask = NodeMask::of(&[NodeKind::LiquidRawTag]);

/// `oncePerFileCheck(tagName)`.
pub fn once_per_file<'a>(tag: &'static str, message: &'static str) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(RawTagCheck { tag, message, skip: 1 }))
}

/// `sectionOrBlockOnlyUnlessAllowed(tagName, { allowSnippets })`.
pub fn section_or_block_only<'a>(
    uri: &str,
    allow_snippets: bool,
    tag: &'static str,
    message: &'static str,
) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    if is_section(uri) || is_block(uri) || (allow_snippets && is_snippet(uri)) {
        return None;
    }
    Some(Box::new(RawTagCheck { tag, message, skip: 0 }))
}

/// `rawTagCheck` and `oncePerFileCheck` differ only in how many tags go unreported.
struct RawTagCheck {
    tag: &'static str,
    message: &'static str,
    skip: u32,
}

impl<'a> LiquidCheck<'a> for RawTagCheck {
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidRawTag(tag) = node else { return };
        if tag.name != self.tag {
            return;
        }
        if self.skip > 0 {
            self.skip -= 1;
            return;
        }
        let p = tag.block_start_position;
        cx.report(LiquidProblem::new(self.message, p.start, p.end));
    }
}
