//! Port of the parts of Prettier's document IR the plugin and the JSON printer use
//! (`src/document` in Prettier 3.9.10, bundled as `doc.mjs`).
//!
//! `Doc::Hardline` and `Doc::Literalline` stand for upstream's `[line, breakParent]`
//! pairs, so the common cases need no allocation. Conditional groups, labels, `trim`
//! and the cursor are not ported: nothing here produces them.

pub mod printer;
pub mod utils;
pub mod width;
mod width_data;

use std::borrow::Cow;

/// `0` means "no id" (a group without `id`, an `ifBreak` without `groupId`).
pub type GroupId = u32;

#[derive(Debug)]
pub struct GroupDoc<'a> {
    pub contents: Doc<'a>,
    pub id: GroupId,
    pub break_: bool,
}

#[derive(Debug)]
pub struct IfBreakDoc<'a> {
    pub break_contents: Doc<'a>,
    pub flat_contents: Doc<'a>,
    pub group_id: GroupId,
}

#[derive(Debug)]
pub enum Doc<'a> {
    /// The empty string.
    Nil,
    Str(&'a str),
    String(Box<str>),
    Concat(Vec<Doc<'a>>),
    Indent(Box<Doc<'a>>),
    /// `align(Number.NEGATIVE_INFINITY, ...)`.
    DedentToRoot(Box<Doc<'a>>),
    Group(Box<GroupDoc<'a>>),
    Fill(Vec<Doc<'a>>),
    IfBreak(Box<IfBreakDoc<'a>>),
    LineSuffix(Box<Doc<'a>>),
    LineSuffixBoundary,
    Line,
    Softline,
    Hardline,
    Literalline,
    /// `markAsRoot(literalline)`: a literal line whose root is the current indent.
    RootedLiteralline,
    BreakParent,
    /// Embedded code whose layout depends on the indent reached at print time.
    Embed(Box<crate::embed::EmbedDoc<'a>>),
}

impl<'a> From<&'a str> for Doc<'a> {
    #[inline]
    fn from(s: &'a str) -> Self {
        if s.is_empty() {
            Doc::Nil
        } else {
            Doc::Str(s)
        }
    }
}

impl From<String> for Doc<'_> {
    fn from(s: String) -> Self {
        if s.is_empty() {
            Doc::Nil
        } else {
            Doc::String(s.into_boxed_str())
        }
    }
}

impl<'a> From<Cow<'a, str>> for Doc<'a> {
    fn from(s: Cow<'a, str>) -> Self {
        match s {
            Cow::Borrowed(s) => s.into(),
            Cow::Owned(s) => s.into(),
        }
    }
}

impl<'a> From<Vec<Doc<'a>>> for Doc<'a> {
    #[inline]
    fn from(parts: Vec<Doc<'a>>) -> Self {
        Doc::Concat(parts)
    }
}

/// `[a, b, ...]`.
#[macro_export]
macro_rules! concat {
    ($($e:expr),* $(,)?) => {
        $crate::doc::Doc::Concat(vec![$($crate::doc::Doc::from($e)),*])
    };
}

#[inline]
pub fn indent<'a>(contents: impl Into<Doc<'a>>) -> Doc<'a> {
    Doc::Indent(Box::new(contents.into()))
}

#[inline]
pub fn dedent_to_root<'a>(contents: impl Into<Doc<'a>>) -> Doc<'a> {
    Doc::DedentToRoot(Box::new(contents.into()))
}

#[inline]
pub fn group<'a>(contents: impl Into<Doc<'a>>) -> Doc<'a> {
    group_with(contents, 0, false)
}

#[inline]
pub fn group_id<'a>(contents: impl Into<Doc<'a>>, id: GroupId) -> Doc<'a> {
    group_with(contents, id, false)
}

#[inline]
pub fn group_with<'a>(contents: impl Into<Doc<'a>>, id: GroupId, break_: bool) -> Doc<'a> {
    Doc::Group(Box::new(GroupDoc { contents: contents.into(), id, break_ }))
}

#[inline]
pub fn if_break<'a>(
    break_contents: impl Into<Doc<'a>>,
    flat_contents: impl Into<Doc<'a>>,
    group_id: GroupId,
) -> Doc<'a> {
    Doc::IfBreak(Box::new(IfBreakDoc {
        break_contents: break_contents.into(),
        flat_contents: flat_contents.into(),
        group_id,
    }))
}

#[inline]
pub fn line_suffix<'a>(contents: impl Into<Doc<'a>>) -> Doc<'a> {
    Doc::LineSuffix(Box::new(contents.into()))
}

/// `join(separator, docs)` where `separator` is rebuilt for each gap.
pub fn join_with<'a>(
    docs: impl IntoIterator<Item = Doc<'a>>,
    mut separator: impl FnMut() -> Doc<'a>,
) -> Doc<'a> {
    let mut parts = Vec::new();
    for (i, d) in docs.into_iter().enumerate() {
        if i > 0 {
            parts.push(separator());
        }
        parts.push(d);
    }
    Doc::Concat(parts)
}

/// `join(hardline, lines)` for plain strings.
pub fn join_hardline<'a>(lines: &[&'a str]) -> Doc<'a> {
    let mut parts = Vec::with_capacity(lines.len() * 2);
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            parts.push(Doc::Hardline);
        }
        parts.push(Doc::from(*l));
    }
    Doc::Concat(parts)
}
