//! `StaticStylesheetAndJavascriptTags`.

use crate::api::*;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(StaticStylesheetAndJavascriptTags))
}

struct StaticStylesheetAndJavascriptTags;

impl<'a> LiquidCheck<'a> for StaticStylesheetAndJavascriptTags {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidRawTag(tag) = node else { return };
        let tag_type = match tag.name {
            "stylesheet" => "CSS",
            "javascript" => "JavaScript",
            _ => return,
        };
        let Node::RawMarkup(body) = &tag.body else { return };
        for child in &body.nodes {
            let liquid_type = match child {
                Node::LiquidVariableOutput(_) => "variable",
                Node::LiquidTag(_) | Node::LiquidRawTag(_) => "tag",
                _ => continue,
            };
            let p = child.position();
            cx.report(LiquidProblem::new(
                format!(
                    "Liquid {liquid_type} found in {tag_type} block. {{% {} %}} tags should only contain static {tag_type} code.",
                    tag.name
                ),
                p.start,
                p.end,
            ));
        }
    }
}
