//! `MaxFileSize` for JSON files.

use super::shared_rules::check_max_file_size;
use crate::api::*;

pub const IMPL: CheckImpl = CheckImpl::Json { create, enter: JsonMask::NONE, exit: JsonMask::NONE };

fn create<'a, 'f>(_cx: &JsonCx<'a, 'f>) -> Option<Box<dyn JsonCheck<'a> + 'a>> {
    Some(Box::new(MaxFileSizeJson))
}

struct MaxFileSizeJson;

impl<'a> JsonCheck<'a> for MaxFileSizeJson {
    fn on_code_path_start(&mut self, cx: &JsonCx<'a, '_>) {
        check_max_file_size(cx);
    }
}
