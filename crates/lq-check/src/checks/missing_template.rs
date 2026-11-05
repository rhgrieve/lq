//! `MissingTemplate`: a Liquid check that asks the file system. Upstream offers no
//! fix or suggestion.

use crate::api::*;
use lq_syntax::{Markup, Node, Position};
use lq_theme::minimatch::minimatch;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::RenderMarkup, NodeKind::LiquidTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(MissingTemplate))
}

struct MissingTemplate;

fn maybe_report_missing(cx: &LiquidCx<'_, '_>, relative_path: &str, position: Position) {
    if cx.does_file_exist(relative_path) {
        return;
    }
    let ignored = cx.settings().strs("ignoreMissing").iter().any(|p| minimatch(relative_path, p));
    if !ignored {
        cx.report(LiquidProblem::new(format!("'{relative_path}' does not exist"), position.start, position.end));
    }
}

impl<'a> LiquidCheck<'a> for MissingTemplate {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        match node {
            Node::RenderMarkup(m) => {
                if let Node::String(snippet) = &m.snippet {
                    let path = format!("snippets/{}.liquid", snippet.value);
                    maybe_report_missing(cx, &path, m.snippet.position());
                }
            }
            Node::LiquidTag(tag) if tag.name == "section" => {
                let Markup::Node(markup) = &tag.markup else { return };
                let Node::SectionMarkup(m) = &**markup else { return };
                if let Node::String(name) = &m.name {
                    let path = format!("sections/{}.liquid", name.value);
                    maybe_report_missing(cx, &path, m.name.position());
                }
            }
            _ => {}
        }
    }
}
