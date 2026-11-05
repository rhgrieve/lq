//! Port of Prettier's comment attachment (`src/main/comments/attach.js`) with the
//! estree `handleComments` that can fire for JSON: every other handler is keyed on a
//! node type the JSON parser never produces.

use super::parser::*;
use super::skip::*;
use crate::printer::string::is_js_ws;

#[derive(Clone, Copy, Default)]
struct Decorated {
    enclosing: Option<Id>,
    preceding: Option<Id>,
    following: Option<Id>,
}

fn child_nodes(ast: &Ast<'_>, id: Id) -> Vec<Id> {
    match &ast.nodes[id as usize].kind {
        Kind::Root { node } => vec![*node],
        Kind::Object { properties } => properties.clone(),
        Kind::Array { elements } => elements.iter().flatten().copied().collect(),
        Kind::Property { key, value } => vec![*key, *value],
        Kind::Unary { argument, .. } => vec![*argument],
        _ => Vec::new(),
    }
}

fn decorate_comment(ast: &Ast<'_>, node: Id, comment: usize, enclosing: Option<Id>) -> Decorated {
    let (comment_start, comment_end) = (ast.comments[comment].start, ast.comments[comment].end);
    let children = child_nodes(ast, node);
    let mut preceding = None;
    let mut following = None;
    let mut left = 0;
    let mut right = children.len();
    while left < right {
        let middle = (left + right) >> 1;
        let child = children[middle];
        let (start, end) = (ast.nodes[child as usize].start, ast.nodes[child as usize].end);
        if start <= comment_start && comment_end <= end {
            return decorate_comment(ast, child, comment, Some(child));
        }
        if end <= comment_start {
            preceding = Some(child);
            left = middle + 1;
            continue;
        }
        if comment_end <= start {
            following = Some(child);
            right = middle;
            continue;
        }
        unreachable!("comments lie between tokens");
    }
    Decorated { enclosing, preceding, following }
}

fn add_leading(ast: &mut Ast<'_>, node: Id, comment: usize) {
    ast.comments[comment].leading = true;
    ast.comments[comment].trailing = false;
    ast.nodes[node as usize].comments.push(comment as u32);
}

fn add_dangling(ast: &mut Ast<'_>, node: Id, comment: usize) {
    ast.comments[comment].leading = false;
    ast.comments[comment].trailing = false;
    ast.nodes[node as usize].comments.push(comment as u32);
}

fn add_trailing(ast: &mut Ast<'_>, node: Id, comment: usize) {
    ast.comments[comment].leading = false;
    ast.comments[comment].trailing = true;
    ast.nodes[node as usize].comments.push(comment as u32);
}

/// `!/[\S\n  ]/.test(text)`.
fn is_all_empty_and_no_line_break(text: &str) -> bool {
    text.chars().all(|c| is_js_ws(c) && !matches!(c, '\n' | '\u{2028}' | '\u{2029}'))
}

fn is_own_line_comment(ast: &Ast<'_>, decorated: &[Decorated], index: usize) -> bool {
    let preceding = decorated[index].preceding;
    let mut start = ast.comments[index].start;
    if preceding.is_some() {
        for i in (0..index).rev() {
            if decorated[i].preceding != preceding
                || !is_all_empty_and_no_line_break(&ast.text[ast.comments[i].end..start])
            {
                break;
            }
            start = ast.comments[i].start;
        }
    }
    has_newline_backwards(ast.text, start)
}

fn is_end_of_line_comment(ast: &Ast<'_>, decorated: &[Decorated], index: usize) -> bool {
    let following = decorated[index].following;
    let mut end = ast.comments[index].end;
    if following.is_some() {
        for (d, comment) in decorated.iter().zip(&ast.comments).skip(index + 1) {
            if d.following != following
                || !is_all_empty_and_no_line_break(&ast.text[end..comment.start])
            {
                break;
            }
            end = comment.end;
        }
    }
    has_newline(ast.text, end)
}

/// `/^[\s(]*$/`.
fn is_gap(gap: &str) -> bool {
    gap.chars().all(|c| is_js_ws(c) || c == '(')
}

fn break_ties(ast: &mut Ast<'_>, ties: &mut Vec<usize>, decorated: &[Decorated]) {
    if ties.is_empty() {
        return;
    }
    let preceding = decorated[ties[0]].preceding.unwrap();
    let following = decorated[ties[0]].following.unwrap();
    let mut gap_end = ast.nodes[following as usize].start;
    let mut first_leading = ties.len();
    while first_leading > 0 {
        let comment = &ast.comments[ties[first_leading - 1]];
        if is_gap(&ast.text[comment.end..gap_end]) {
            gap_end = comment.start;
        } else {
            break;
        }
        first_leading -= 1;
    }
    for (i, &comment) in ties.iter().enumerate() {
        if i < first_leading {
            add_trailing(ast, preceding, comment);
        } else {
            add_leading(ast, following, comment);
        }
    }
    for node in [preceding, following] {
        let mut comments = std::mem::take(&mut ast.nodes[node as usize].comments);
        comments.sort_by_key(|c| ast.comments[*c as usize].start);
        ast.nodes[node as usize].comments = comments;
    }
    ties.clear();
}

/// `isTypeCastComment`: `/** @type ... */`.
fn is_type_cast_comment(c: &Comment<'_>) -> bool {
    if !c.block || !c.value.starts_with('*') {
        return false;
    }
    for marker in ["@type", "@satisfies"] {
        let mut from = 0;
        while let Some(i) = c.value[from..].find(marker) {
            let after = from + i + marker.len();
            let boundary = c.value[after..]
                .chars()
                .next()
                .is_none_or(|n| !(n.is_ascii_alphanumeric() || n == '_'));
            if boundary {
                return true;
            }
            from = after;
        }
    }
    false
}

/// `attachComments`.
pub fn attach_comments(ast: &mut Ast<'_>) {
    if ast.comments.is_empty() {
        return;
    }
    let root = ast.root;
    let decorated: Vec<Decorated> =
        (0..ast.comments.len()).map(|i| decorate_comment(ast, root, i, None)).collect();
    let mut ties: Vec<usize> = Vec::new();
    for index in 0..decorated.len() {
        let Decorated { enclosing, preceding, following } = decorated[index];
        let dangle_target = enclosing.unwrap_or(root);
        if is_own_line_comment(ast, &decorated, index) {
            if let Some(f) = following {
                add_leading(ast, f, index);
            } else if let Some(p) = preceding {
                add_trailing(ast, p, index);
            } else {
                add_dangling(ast, dangle_target, index);
            }
        } else if is_end_of_line_comment(ast, &decorated, index) {
            // handleTypeCastComments, then handlePropertyComments.
            if let (Some(f), true) = (following, is_type_cast_comment(&ast.comments[index])) {
                add_leading(ast, f, index);
            } else if let Some(e) = enclosing
                .filter(|e| matches!(ast.nodes[*e as usize].kind, Kind::Property { .. }))
            {
                add_leading(ast, e, index);
            } else if let Some(p) = preceding {
                add_trailing(ast, p, index);
            } else if let Some(f) = following {
                add_leading(ast, f, index);
            } else {
                add_dangling(ast, dangle_target, index);
            }
        } else if let (Some(_), Some(f)) = (preceding, following) {
            if let Some(&last) = ties.last() {
                if decorated[last].following != Some(f) {
                    break_ties(ast, &mut ties, &decorated);
                }
            }
            ties.push(index);
        } else if let Some(p) = preceding {
            add_trailing(ast, p, index);
        } else if let Some(f) = following {
            add_leading(ast, f, index);
        } else {
            add_dangling(ast, dangle_target, index);
        }
    }
    break_ties(ast, &mut ties, &decorated);
}
