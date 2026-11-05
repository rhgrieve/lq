//! `RequiredLayoutThemeObject`.

use crate::api::*;
use crate::check_utils::is_html_tag;
use lq_syntax::{Node, Position};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::VariableLookup, NodeKind::HtmlElement]),
    exit: NodeMask::NONE,
};

/// `/^layout\/[^/]+\.liquid$/i`.
fn is_layout_path(relative_path: &str) -> bool {
    const PREFIX: &str = "layout/";
    const SUFFIX: &str = ".liquid";
    let bytes = relative_path.as_bytes();
    if bytes.len() < PREFIX.len() + 1 + SUFFIX.len()
        || !bytes[..PREFIX.len()].eq_ignore_ascii_case(PREFIX.as_bytes())
        || !bytes[bytes.len() - SUFFIX.len()..].eq_ignore_ascii_case(SUFFIX.as_bytes())
    {
        return false;
    }
    !bytes[PREFIX.len()..bytes.len() - SUFFIX.len()].contains(&b'/')
}

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    if !is_layout_path(&cx.to_relative_path(cx.uri())) {
        return None;
    }
    Some(Box::<RequiredLayoutThemeObject>::default())
}

#[derive(Default)]
struct RequiredLayoutThemeObject {
    has_header: bool,
    has_layout: bool,
    /// `(position, blockEndPosition.start)`.
    head: Option<(Position, u32)>,
    body: Option<(Position, u32)>,
}

fn report_missing(cx: &LiquidCx<'_, '_>, object: &str, insertion: Option<(Position, u32)>) {
    let tag = format!("{{{{ {object} }}}}");
    let message = format!("The required object '{tag}' is missing in layout/theme.liquid");
    let (start, end) = insertion.map_or((0, 0), |(p, _)| (p.start, p.end));
    let mut problem = LiquidProblem::new(message, start, end);
    if let Some((_, at)) = insertion {
        problem = problem.fix(|c| c.insert(at, tag));
    }
    cx.report(problem);
}

impl<'a> LiquidCheck<'a> for RequiredLayoutThemeObject {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], _cx: &LiquidCx<'a, '_>) {
        match node {
            Node::VariableLookup(lookup) => match lookup.name {
                Some("content_for_header") => self.has_header = true,
                Some("content_for_layout") => self.has_layout = true,
                _ => {}
            },
            Node::HtmlElement(element) => {
                if is_html_tag(node, "head") {
                    self.head = Some((node.position(), element.block_end_position.start));
                } else if is_html_tag(node, "body") {
                    self.body = Some((node.position(), element.block_end_position.start));
                }
            }
            _ => {}
        }
    }

    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {
        if !self.has_header {
            report_missing(cx, "content_for_header", self.head);
        }
        if !self.has_layout {
            report_missing(cx, "content_for_layout", self.body);
        }
    }
}
