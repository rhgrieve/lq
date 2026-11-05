//! `MissingTemplate`: a Liquid check that asks the file system. Upstream offers no
//! fix or suggestion.

use crate::api::*;
use crate::check_utils::{closest, did_you_mean};
use lq_syntax::{Markup, Node, Position};
use lq_theme::minimatch::minimatch;
use std::cell::OnceCell;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::RenderMarkup, NodeKind::LiquidTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::<MissingTemplate>::default())
}

#[derive(Default)]
struct MissingTemplate {
    /// Only read once something is missing, and then only once per file.
    snippets: OnceCell<Vec<String>>,
    sections: OnceCell<Vec<String>>,
}

impl MissingTemplate {
    /// The nearest file of that kind, or else the likely reason there's none.
    fn missing_note(&self, cx: &LiquidCx<'_, '_>, relative_path: &str) -> String {
        let (directory, file) = relative_path.split_once('/').unwrap_or(("", relative_path));
        let name = file.strip_suffix(".liquid").unwrap_or(file);
        let cell = if directory == "sections" { &self.sections } else { &self.snippets };
        let names = cell.get_or_init(|| liquid_names(cx, directory));
        match closest(name, names.iter().map(String::as_str)) {
            Some(found) => did_you_mean(found),
            None => "if a build step generates this file, list it in this check's ignoreMissing setting".to_string(),
        }
    }

    fn maybe_report_missing(&self, cx: &LiquidCx<'_, '_>, relative_path: &str, position: Position) {
        if cx.does_file_exist(relative_path) {
            return;
        }
        let ignored = cx.settings().strs("ignoreMissing").iter().any(|p| minimatch(relative_path, p));
        if !ignored {
            cx.report(
                LiquidProblem::new(format!("'{relative_path}' does not exist"), position.start, position.end)
                    .hints_if(cx.wants_hints(), |p| p.label("not found").note(self.missing_note(cx, relative_path))),
            );
        }
    }
}

/// The Liquid files in a directory, without their extension.
fn liquid_names(cx: &LiquidCx<'_, '_>, directory: &str) -> Vec<String> {
    let files = cx.fs().read_directory(&cx.to_uri(directory)).unwrap_or_default();
    files
        .iter()
        .filter(|(uri, _)| uri.ends_with(".liquid"))
        .map(|(uri, _)| lq_json::uri::basename(uri, Some(".liquid")))
        .collect()
}

impl<'a> LiquidCheck<'a> for MissingTemplate {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        match node {
            Node::RenderMarkup(m) => {
                if let Node::String(snippet) = &m.snippet {
                    let path = format!("snippets/{}.liquid", snippet.value);
                    self.maybe_report_missing(cx, &path, m.snippet.position());
                }
            }
            Node::LiquidTag(tag) if tag.name == "section" => {
                let Markup::Node(markup) = &tag.markup else { return };
                let Node::SectionMarkup(m) = &**markup else { return };
                if let Node::String(name) = &m.name {
                    let path = format!("sections/{}.liquid", name.value);
                    self.maybe_report_missing(cx, &path, m.name.position());
                }
            }
            _ => {}
        }
    }
}
