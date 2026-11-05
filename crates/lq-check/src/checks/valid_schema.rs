//! `ValidSchema`.

use super::shared_schema::schema_raw_tag;
use crate::api::*;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check))
}

struct Check;

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Some(tag) = schema_raw_tag(node) else { return };
        if !cx.has_validate_json() {
            return;
        }
        let offset = tag.block_start_position.end;
        let json = &cx.source()[offset as usize..tag.block_end_position.start as usize];
        // The file's `ThemeSchema` already validates this text for `validSchema`.
        let shared = cx
            .schema()
            .filter(|s| s.value == json && s.is_of_uri(cx.uri()))
            .and_then(|s| s.shared_validation());
        let owned;
        let result = match shared {
            Some(r) => r,
            None => {
                let Some(r) = cx.validate_json(cx.uri(), json) else { return };
                owned = r;
                &owned
            }
        };
        match result {
            Ok(problems) => {
                for problem in problems {
                    cx.report(LiquidProblem::new(problem.message.clone(), offset + problem.start, offset + problem.end));
                }
            }
            Err(e) => cx.fail(e.clone()),
        }
    }
}
