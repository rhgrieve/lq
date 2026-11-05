//! `SchemaSectionOrBlockOnly` from `checks/raw-tags`.

use super::shared_syntax::{section_or_block_only, RAW_TAG_ENTER};
use crate::api::*;

pub const IMPL: CheckImpl =
    CheckImpl::Liquid { create, enter: RAW_TAG_ENTER, exit: NodeMask::NONE };

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    section_or_block_only(cx.uri(), false, "schema", "{% schema %} is only valid in section or block files.")
}
