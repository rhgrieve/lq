//! `StylesheetOncePerFile` from `checks/raw-tags`.

use super::shared_syntax::{once_per_file, RAW_TAG_ENTER};
use crate::api::*;

pub const IMPL: CheckImpl =
    CheckImpl::Liquid { create, enter: RAW_TAG_ENTER, exit: NodeMask::NONE };

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    once_per_file("stylesheet", "{% stylesheet %} can only appear once per file.")
}
