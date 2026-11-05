//! `ValidBlockTagPlacement`.

use super::shared_args::valid_block_markup;
use crate::api::*;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check))
}

struct Check;

/// `[^/]+\.liquid$`.
fn is_liquid_file_name(segment: &str) -> bool {
    segment.len() > ".liquid".len() && segment.ends_with(".liquid") && !segment.contains('/')
}

/// `TEMPLATE_FILE_PATTERN` or `LAYOUT_FILE_PATTERN`.
fn is_allowed_file(relative_path: &str) -> bool {
    if let Some(rest) = relative_path.strip_prefix("templates/") {
        // `^templates\/(?:[^/]+\/)*[^/]+\.liquid$`.
        return match rest.rsplit_once('/') {
            Some((dirs, file)) => is_liquid_file_name(file) && dirs.split('/').all(|d| !d.is_empty()),
            None => is_liquid_file_name(rest),
        };
    }
    // `^layout\/[^/]+\.liquid$`.
    relative_path.strip_prefix("layout/").is_some_and(is_liquid_file_name)
}

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidTag(tag) = node else { return };
        if valid_block_markup(tag, cx.source()).is_none() || is_allowed_file(&cx.to_relative_path(cx.uri())) {
            return;
        }
        let p = node.position();
        cx.report(LiquidProblem::new(
            "The 'block' tag can only be used in templates/**/*.liquid and layout/*.liquid.",
            p.start,
            p.end,
        ));
    }
}
