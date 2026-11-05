//! `doc.utils`: `propagateBreaks`, `removeLines`, `replaceEndOfLine`.

use super::{group_with, Doc};

/// `propagateBreaks`. Returns whether `doc` forces its enclosing group to break.
/// Docs here are trees, so upstream's visited-set is not needed.
pub fn propagate_breaks(doc: &mut Doc<'_>) -> bool {
    match doc {
        // An embed stands for a shape holding hardlines, as does its fallback.
        Doc::Hardline | Doc::Literalline | Doc::RootedLiteralline | Doc::BreakParent | Doc::Embed(_) => {
            true
        }
        Doc::Concat(parts) | Doc::Fill(parts) => {
            let mut breaks = false;
            for p in parts {
                breaks |= propagate_breaks(p);
            }
            breaks
        }
        Doc::Indent(c) | Doc::DedentToRoot(c) | Doc::LineSuffix(c) => propagate_breaks(c),
        Doc::Group(g) => {
            if propagate_breaks(&mut g.contents) {
                g.break_ = true;
            }
            g.break_
        }
        Doc::IfBreak(i) => {
            let a = propagate_breaks(&mut i.break_contents);
            let b = propagate_breaks(&mut i.flat_contents);
            a || b
        }
        Doc::Nil
        | Doc::Str(_)
        | Doc::String(_)
        | Doc::Line
        | Doc::Softline
        | Doc::LineSuffixBoundary => false,
    }
}

/// `removeLines`.
pub fn remove_lines(doc: Doc<'_>) -> Doc<'_> {
    match doc {
        Doc::Line => Doc::Str(" "),
        Doc::Softline => Doc::Nil,
        Doc::IfBreak(i) => remove_lines(i.flat_contents),
        Doc::Concat(parts) => Doc::Concat(parts.into_iter().map(remove_lines).collect()),
        Doc::Fill(parts) => Doc::Fill(parts.into_iter().map(remove_lines).collect()),
        Doc::Indent(c) => Doc::Indent(Box::new(remove_lines(*c))),
        Doc::DedentToRoot(c) => Doc::DedentToRoot(Box::new(remove_lines(*c))),
        Doc::LineSuffix(c) => Doc::LineSuffix(Box::new(remove_lines(*c))),
        Doc::Group(g) => {
            let g = *g;
            group_with(remove_lines(g.contents), g.id, g.break_)
        }
        other => other,
    }
}

/// `replaceEndOfLine(text)`, with `literalline` as the replacement.
pub fn replace_end_of_line(text: &str) -> Doc<'_> {
    if !text.contains('\n') {
        return Doc::from(text);
    }
    let mut parts = Vec::new();
    for (i, l) in text.split('\n').enumerate() {
        if i > 0 {
            parts.push(Doc::Literalline);
        }
        parts.push(Doc::from(l));
    }
    Doc::Concat(parts)
}
