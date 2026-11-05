//! Port of `src/printer/**` of @shopify/prettier-plugin-liquid 1.11.3.
//!
//! The plugin augments AST nodes in place. Here those fields live in `Ctx::info`, a side
//! table indexed by `NodeId`; `Node`s stay immutable and are looked up by id.

pub mod preprocess;
pub mod print;
pub mod printer_liquid_html;
pub mod string;
pub mod utils;

use crate::doc::GroupId;
use crate::options::Options;
use lq_syntax::Node;
use std::cell::{Cell, RefCell};

pub type N<'a> = &'a Node<'a>;

const NONE: u32 = u32::MAX;

pub const IS_DANGLING_WS_SENSITIVE: u16 = 1 << 0;
pub const IS_INDENTATION_SENSITIVE: u16 = 1 << 1;
pub const IS_WHITESPACE_SENSITIVE: u16 = 1 << 2;
pub const IS_LEADING_WS_SENSITIVE: u16 = 1 << 3;
pub const IS_TRAILING_WS_SENSITIVE: u16 = 1 << 4;
pub const HAS_LEADING_WS: u16 = 1 << 5;
pub const HAS_TRAILING_WS: u16 = 1 << 6;
pub const HAS_DANGLING_WS: u16 = 1 << 7;
pub const IS_ATTRIBUTE: u16 = 1 << 8;

#[derive(Clone, Copy)]
pub struct NodeInfo<'a> {
    pub parent: u32,
    pub prev: u32,
    pub next: u32,
    pub first_child: u32,
    pub last_child: u32,
    pub css_display: &'a str,
    pub css_whitespace: &'a str,
    pub flags: u16,
}

pub struct Ctx<'a> {
    pub source: &'a str,
    pub options: &'a Options,
    pub nodes: Vec<Option<N<'a>>>,
    pub info: Vec<NodeInfo<'a>>,
    next_group_id: Cell<GroupId>,
    /// The first error thrown while printing, which upstream would surface.
    pub error: RefCell<Option<String>>,
}

/// `Symbol('force-no-break')` and `Symbol('force-break')`. These never reach a doc:
/// `ifBreakChain` resolves them.
pub const FORCE_FLAT_GROUP_ID: GroupId = u32::MAX;
pub const FORCE_BREAK_GROUP_ID: GroupId = u32::MAX - 1;

/// `symbol[] | symbol | undefined`. `undefined` is represented as the one id `0`, which
/// produces an `ifBreak` without `groupId`, as upstream's `[undefined]` does.
#[derive(Clone, Copy, Debug)]
pub struct GroupIds {
    ids: [GroupId; 3],
    len: u8,
}

impl GroupIds {
    pub const UNDEFINED: GroupIds = GroupIds { ids: [0; 3], len: 1 };

    #[inline]
    pub fn one(id: GroupId) -> Self {
        GroupIds { ids: [id, 0, 0], len: 1 }
    }

    #[inline]
    pub fn from_vec(v: &[GroupId]) -> Self {
        let mut ids = [0; 3];
        ids[..v.len()].copy_from_slice(v);
        GroupIds { ids, len: v.len() as u8 }
    }

    #[inline]
    pub fn as_slice(&self) -> &[GroupId] {
        &self.ids[..self.len as usize]
    }
}

/// `LiquidPrinterArgs`.
#[derive(Clone, Copy, Debug)]
pub struct Args {
    pub leading_space_group_id: GroupIds,
    pub trailing_space_group_id: GroupIds,
    pub is_liquid_statement: bool,
    pub truncate: bool,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            leading_space_group_id: GroupIds::UNDEFINED,
            trailing_space_group_id: GroupIds::UNDEFINED,
            is_liquid_statement: false,
            truncate: false,
        }
    }
}

impl<'a> Ctx<'a> {
    pub fn new(source: &'a str, options: &'a Options, node_count: usize) -> Self {
        Ctx {
            source,
            options,
            nodes: vec![None; node_count],
            info: vec![
                NodeInfo {
                    parent: NONE,
                    prev: NONE,
                    next: NONE,
                    first_child: NONE,
                    last_child: NONE,
                    css_display: "should not be relevant",
                    css_whitespace: "should not be relevant",
                    flags: 0,
                };
                node_count
            ],
            next_group_id: Cell::new(1),
            error: RefCell::new(None),
        }
    }

    /// A new `Symbol(...)`.
    #[inline]
    pub fn symbol(&self) -> GroupId {
        let id = self.next_group_id.get();
        self.next_group_id.set(id + 1);
        id
    }

    pub fn fail(&self, message: String) {
        let mut e = self.error.borrow_mut();
        if e.is_none() {
            *e = Some(message);
        }
    }

    #[inline]
    pub fn get(&self, n: N<'a>) -> &NodeInfo<'a> {
        &self.info[n.id().0 as usize]
    }

    #[inline]
    fn resolve(&self, id: u32) -> Option<N<'a>> {
        if id == NONE {
            None
        } else {
            self.nodes[id as usize]
        }
    }

    #[inline]
    pub fn parent(&self, n: N<'a>) -> Option<N<'a>> {
        self.resolve(self.get(n).parent)
    }

    #[inline]
    pub fn prev(&self, n: N<'a>) -> Option<N<'a>> {
        self.resolve(self.get(n).prev)
    }

    #[inline]
    pub fn next(&self, n: N<'a>) -> Option<N<'a>> {
        self.resolve(self.get(n).next)
    }

    #[inline]
    pub fn first_child(&self, n: N<'a>) -> Option<N<'a>> {
        self.resolve(self.get(n).first_child)
    }

    #[inline]
    pub fn last_child(&self, n: N<'a>) -> Option<N<'a>> {
        self.resolve(self.get(n).last_child)
    }

    #[inline]
    pub fn has_flag(&self, n: N<'a>, flag: u16) -> bool {
        self.get(n).flags & flag != 0
    }

    #[inline]
    pub fn same(a: N<'a>, b: N<'a>) -> bool {
        a.id() == b.id()
    }
}
