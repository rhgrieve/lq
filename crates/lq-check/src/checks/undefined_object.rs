//! `UndefinedObject`.

use crate::api::*;
use crate::check_utils::{closest, did_you_mean};
use crate::block_parameters::BLOCK_CONTENT_PARAMETER;
use crate::check_utils::is_within_unparsed_raw_tag;
use crate::liquid_doc::has_liquid_doc;
use crate::to_schema::is_block;
use lq_syntax::{Markup, Node, Position};
use lq_theme::config::Mode;
use std::collections::{HashMap, HashSet};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidDocParamNode, NodeKind::LiquidTag, NodeKind::VariableLookup]),
    exit: NodeMask::NONE,
};

const CHECKOUT_CONTEXTUAL_OBJECTS: [&str; 13] = [
    "locale",
    "direction",
    "skip_to_content_link",
    "checkout_html_classes",
    "checkout_stylesheets",
    "checkout_scripts",
    "content_for_logo",
    "breadcrumb",
    "order_summary_toggle",
    "content_for_order_summary",
    "alternative_payment_methods",
    "content_for_footer",
    "tracking_code",
];
const BLOCK_CONTEXTUAL_OBJECTS: [&str; 4] = ["app", "section", "recommendations", "block"];
const SECTION_CONTEXTUAL_OBJECTS: [&str; 4] = ["section", "predictive_search", "recommendations", "comment"];
const SNIPPET_CONTEXTUAL_OBJECTS: [&str; 3] = ["app", "section", "block"];

/// `getContextualObjects(relativePath, mode)`.
fn contextual_objects(relative_path: &str, mode: Mode) -> &'static [&'static str] {
    if relative_path.starts_with("layout/checkout.liquid") {
        &CHECKOUT_CONTEXTUAL_OBJECTS
    } else if relative_path.starts_with("sections/") {
        &SECTION_CONTEXTUAL_OBJECTS
    } else if relative_path.starts_with("blocks/") {
        &BLOCK_CONTEXTUAL_OBJECTS
    } else if relative_path.starts_with("snippets/") {
        if mode == Mode::App {
            &BLOCK_CONTEXTUAL_OBJECTS
        } else {
            &SNIPPET_CONTEXTUAL_OBJECTS
        }
    } else {
        &[]
    }
}

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    let ast = cx.file().liquid_ast().ok()?;
    let relative_path = cx.to_relative_path(cx.uri());
    // A doc tag needs its name in the source: most snippets skip the walk.
    if relative_path.starts_with("snippets/") && (!cx.source().contains("doc") || !has_liquid_doc(ast)) {
        return None;
    }
    cx.docset()?;
    Some(Box::new(UndefinedObject {
        is_blocks_path: relative_path.starts_with("blocks/"),
        contextual: contextual_objects(&relative_path, cx.mode()),
        scoped: HashMap::new(),
        file_scoped: HashSet::new(),
        variables: Vec::new(),
    }))
}

#[derive(Clone, Copy)]
struct Scope {
    start: Option<u32>,
    end: Option<u32>,
}

struct UndefinedObject<'a> {
    is_blocks_path: bool,
    contextual: &'static [&'static str],
    scoped: HashMap<&'a str, Vec<Scope>>,
    /// LiquidDoc parameters.
    file_scoped: HashSet<&'a str>,
    variables: Vec<(&'a str, Position)>,
}

impl<'a> UndefinedObject<'a> {
    fn index(&mut self, name: &'a str, start: Option<u32>, end: Option<u32>) {
        if !name.is_empty() {
            self.scoped.entry(name).or_default().push(Scope { start, end });
        }
    }

    fn index_tag(&mut self, node: &'a Node<'a>) {
        let Node::LiquidTag(tag) = node else { return };
        let position = node.position();
        let block_start = tag.block_start_position.end;
        let block_end = tag.block_end_position;
        let markup = match &tag.markup {
            Markup::Node(markup) => Some(&**markup),
            _ => None,
        };
        match (tag.name, markup) {
            ("assign", Some(Node::AssignMarkup(m))) => self.index(m.name, Some(block_start), None),
            ("capture", Some(Node::VariableLookup(l))) => {
                if let Some(name) = l.name {
                    self.index(name, block_end.map(|p| p.end), None);
                }
            }
            ("form" | "paginate", _) => self.index(tag.name, Some(block_start), block_end.map(|p| p.start)),
            ("layout", _) => self.index("none", Some(position.start), Some(position.end)),
            ("increment" | "decrement", Some(Node::VariableLookup(l))) => {
                if let Some(name) = l.name {
                    self.index(name, Some(position.start), None);
                }
            }
            ("for" | "tablerow", Some(Node::ForMarkup(m))) => {
                let end = block_end.map(|p| p.start);
                self.index(m.variable_name, Some(block_start), end);
                self.index(if tag.name == "for" { "forloop" } else { "tablerowloop" }, Some(block_start), end);
            }
            _ => {}
        }
    }

    fn is_scoped(&self, name: &str, position: Position) -> bool {
        self.scoped.get(name).is_some_and(|scopes| {
            scopes.iter().any(|scope| {
                // Upstream tests `!scope.start` and `!scope.end`: an offset of 0 is unset.
                scope.start.is_none_or(|s| s == 0 || position.start > s)
                    && scope.end.is_none_or(|e| e == 0 || position.start < e)
            })
        })
    }
}

impl<'a> LiquidCheck<'a> for UndefinedObject<'a> {
    fn enter(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], _cx: &LiquidCx<'a, '_>) {
        match node {
            Node::LiquidDocParamNode(param) => {
                if let Node::TextNode(text) = &param.param_name {
                    if !text.value.is_empty() {
                        self.file_scoped.insert(&text.value);
                    }
                }
            }
            Node::LiquidTag(_) => {
                if !is_within_unparsed_raw_tag(ancestors) {
                    self.index_tag(node);
                }
            }
            Node::VariableLookup(lookup) => {
                if is_within_unparsed_raw_tag(ancestors) {
                    return;
                }
                if matches!(ancestors.last(), Some(Node::LiquidTag(t)) if t.name == "capture") {
                    return;
                }
                if let Some(name) = lookup.name.filter(|n| !n.is_empty()) {
                    self.variables.push((name, node.position()));
                }
            }
            _ => {}
        }
    }

    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {
        if self.variables.is_empty() {
            return;
        }
        let Some(docset) = cx.docset() else { return };
        let contextual = self.contextual;
        let globals: HashSet<&str> = docset
            .objects()
            .iter()
            .filter(|o| {
                contextual.contains(&o.name)
                    || match &o.access {
                        None => true,
                        Some(access) => access.global || !access.template.is_empty(),
                    }
            })
            .map(|o| o.name)
            .collect();
        let block_parameters = if is_block(cx.uri()) {
            cx.block_parameters(&lq_json::uri::basename(cx.uri(), Some(".liquid")))
        } else {
            None
        };
        for &(name, position) in &self.variables {
            let defined = (self.is_blocks_path && name == BLOCK_CONTENT_PARAMETER)
                || self.file_scoped.contains(name)
                || globals.contains(name)
                || block_parameters.as_ref().is_some_and(|p| p.get(name).is_some())
                || self.is_scoped(name, position);
            if !defined {
                cx.report(
                    LiquidProblem::new(format!("Unknown object '{name}' used."), position.start, position.end).hints_if(cx.wants_hints(), |p| {
                        let known = self.file_scoped.iter().copied().chain(self.scoped.keys().copied()).chain(globals.iter().copied());
                        p.label("not defined").note_opt(closest(name, known).map(did_you_mean))
                    }),
                );
            }
        }
    }
}
