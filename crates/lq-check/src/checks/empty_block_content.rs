//! `EmptyBlockContent`.

use crate::api::*;
use lq_json::Value;
use lq_syntax::{Markup, Node, Position};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::<Check>::default())
}

#[derive(Default)]
struct Check {
    /// `contentForBlocksLocation`, once set: the last such tag.
    location: Option<Position>,
}

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], _cx: &LiquidCx<'a, '_>) {
        let Node::LiquidTag(tag) = node else { return };
        if tag.name != "content_for" {
            return;
        }
        let Markup::Node(markup) = &tag.markup else { return };
        let Node::ContentForMarkup(markup) = &**markup else { return };
        if matches!(&markup.content_for_type, Node::String(s) if s.value == "blocks") {
            self.location = Some(tag.block_start_position);
        }
    }

    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {
        let Some(location) = self.location else { return };
        let Some(schema) = cx.schema() else { return };
        let Some(valid) = schema.valid_value() else { return };
        if schema.ast.is_err() {
            return;
        }
        let message = match valid.get("blocks") {
            Some(blocks) if blocks.is_truthy() => match blocks {
                // `blocks.length === 0`; a truthy string or an object has no zero length.
                Value::Array(a) if a.is_empty() => "The 'content_for \"blocks\"' tag is present, but the blocks array is empty.",
                _ => return,
            },
            _ => "The 'content_for \"blocks\"' tag is present, but the blocks array is not defined.",
        };
        cx.report(LiquidProblem::new(message, location.start, location.end));
    }
}
