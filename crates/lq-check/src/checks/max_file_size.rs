//! `MaxFileSize` for Liquid files.

use super::shared_rules::check_max_file_size;
use crate::api::*;

pub const IMPL: CheckImpl = CheckImpl::Liquid { create, enter: NodeMask::NONE, exit: NodeMask::NONE };

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(MaxFileSize))
}

struct MaxFileSize;

impl<'a> LiquidCheck<'a> for MaxFileSize {
    fn on_code_path_start(&mut self, cx: &LiquidCx<'a, '_>) {
        check_max_file_size(cx);
    }
}
