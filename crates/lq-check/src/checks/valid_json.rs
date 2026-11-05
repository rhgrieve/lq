//! `ValidJSON`: the JSON validator's diagnostics, which also cover syntax errors.

use crate::api::*;

pub const IMPL: CheckImpl = CheckImpl::Json { create, enter: JsonMask::NONE, exit: JsonMask::NONE };

fn create<'a, 'f>(cx: &JsonCx<'a, 'f>) -> Option<Box<dyn JsonCheck<'a> + 'a>> {
    if !cx.has_validate_json() {
        return None;
    }
    Some(Box::new(ValidJson))
}

struct ValidJson;

impl<'a> JsonCheck<'a> for ValidJson {
    fn on_code_path_start(&mut self, cx: &JsonCx<'a, '_>) {
        match cx.validate_json(cx.uri(), cx.source()) {
            None => {}
            Some(Err(error)) => cx.fail(error),
            // `report` reads only the message and offsets: the check's severity wins.
            Some(Ok(problems)) => {
                for p in problems {
                    cx.report(JsonProblem::new(p.message, p.start, p.end));
                }
            }
        }
    }
}
