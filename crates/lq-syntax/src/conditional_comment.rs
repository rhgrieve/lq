//! `getConditionalComment`.

use crate::js;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConditionalComment<'a> {
    pub start_tag: &'a str,
    pub body: &'a str,
    pub end_tag: &'a str,
}

const END_TAG: &str = "<![endif]-->";

/// Hand-written equivalent of
/// `/(<!--\[if[^\]]*]>)((.|\n)*)(<!\[endif\]-->)$/`. `.` excludes `\r`, U+2028
/// and U+2029, so a body holding one of those fails that candidate; a later
/// `<!--[if` can still match.
pub fn get_conditional_comment(comment: &str) -> Option<ConditionalComment<'_>> {
    if !comment.ends_with(END_TAG) {
        return None;
    }
    let end_start = comment.len() - END_TAG.len();
    let mut from = 0;
    while let Some(i) = comment[from..].find("<!--[if") {
        let open = from + i;
        from = open + 1;
        let after = open + "<!--[if".len();
        let Some(close) = memchr::memchr(b']', &comment.as_bytes()[after..]) else { continue };
        let close = after + close;
        if comment.as_bytes().get(close + 1) != Some(&b'>') {
            continue;
        }
        let body_start = close + 2;
        if body_start > end_start {
            continue;
        }
        let body = &comment[body_start..end_start];
        if body.contains(['\r', '\u{2028}', '\u{2029}']) {
            continue;
        }
        return Some(ConditionalComment {
            start_tag: &comment[open..body_start],
            body: js::trim(body),
            end_tag: END_TAG,
        });
    }
    None
}
