//! `UnsupportedFilterArguments`.

use crate::api::*;
use lq_syntax::{Markup, Node};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(UnsupportedFilterArguments))
}

struct UnsupportedFilterArguments;

/// `indexOfFilterPipe(markup)`: the first `|` outside a quoted string.
fn index_of_filter_pipe(markup: &str) -> Option<usize> {
    let mut quote = None;
    for (i, b) in markup.bytes().enumerate() {
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                }
            }
            None => match b {
                b'\'' | b'"' => quote = Some(b),
                b'|' => return Some(i),
                _ => {}
            },
        }
    }
    None
}

impl<'a> LiquidCheck<'a> for UnsupportedFilterArguments {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidTag(tag) = node else { return };
        if !matches!(tag.name, "render" | "include" | "content_for") {
            return;
        }
        let Markup::Str(markup) = tag.markup else { return };
        let Some(pipe) = index_of_filter_pipe(markup) else { return };
        let tag_start = node.position().start as usize;
        let Some(offset) = cx.source().get(tag_start..).and_then(|s| s.find(markup)) else { return };
        let markup_start = tag_start + offset;
        cx.report(LiquidProblem::new(
            format!(
                "Filters cannot be used on arguments passed to the '{}' tag. Apply the filter beforehand (e.g. with {{% assign %}}) and pass the result instead.",
                tag.name
            ),
            (markup_start + pipe) as u32,
            (markup_start + markup.len()) as u32,
        ));
    }
}
