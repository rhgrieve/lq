//! `UniqueStaticBlockId`.

use crate::api::*;
use crate::check_utils::{is_content_for_block, REQUIRED_CONTENT_FOR_ARGUMENTS, RESERVED_CONTENT_FOR_ARGUMENTS};
use lq_syntax::{Markup, Node};
use std::collections::HashSet;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check { used: HashSet::new() }))
}

struct Check {
    used: HashSet<String>,
}

fn is_framework_arg(name: &str) -> bool {
    REQUIRED_CONTENT_FOR_ARGUMENTS.contains(&name) || RESERVED_CONTENT_FOR_ARGUMENTS.contains(&name)
}

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidTag(tag) = node else { return };
        if tag.name != "content_for" || !is_content_for_block(&tag.markup) {
            return;
        }
        let Markup::Node(markup) = &tag.markup else { return };
        let Node::ContentForMarkup(markup) = &**markup else { return };
        let named = || markup.args.iter().filter_map(|a| match a {
            Node::NamedArgument(n) => Some(n),
            _ => None,
        });
        let Some(id_node) = named().find(|a| a.name == "id") else { return };
        let Node::String(id) = &id_node.value else { return };

        let mut parts: Vec<String> = Vec::new();
        for arg in named().filter(|a| !is_framework_arg(&a.name)) {
            let Node::String(value) = &arg.value else { return };
            parts.push(format!("{}={}", arg.name, value.value));
        }
        // JS's default sort compares UTF-16 code units.
        parts.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        let key = format!("{}::{}", id.value, parts.join(","));
        if self.used.contains(&key) {
            cx.report(
                LiquidProblem::new(
                    format!("The id '{}' is already being used by another static block", id.value),
                    id.position.start,
                    id.position.end,
                )
                .with_empty_suggestions(),
            );
        } else {
            self.used.insert(key);
        }
    }
}
