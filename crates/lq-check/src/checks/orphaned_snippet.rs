//! `OrphanedSnippet`.

use crate::api::*;
use crate::to_schema::is_snippet;

pub const IMPL: CheckImpl = CheckImpl::Liquid { create, enter: NodeMask::NONE, exit: NodeMask::NONE };

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check))
}

struct Check;

impl<'a> LiquidCheck<'a> for Check {
    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {
        if !cx.has_references() || !is_snippet(cx.uri()) {
            return;
        }
        if cx.references(cx.uri()).is_empty() {
            cx.report(LiquidProblem::new("This snippet is not referenced by any other files", 0, 1));
        }
    }
}
