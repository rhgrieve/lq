//! `ValidScopedCSSClass`.

use super::shared_schema::ThemeCssClasses;
use crate::api::*;
use crate::context_utils::recursive_read_directory;
use crate::utils::styles::{collect_used_classes, collect_used_classes_from_svg, extract_css_class_names, UsedClass};
use crate::utils::traversal::{all_snippet_descendant_uris, ancestor_uris, rendered_snippet_uris};
use lq_syntax::Node;
use std::collections::{BTreeSet, HashSet};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[
        NodeKind::LiquidRawTag,
        NodeKind::HtmlElement,
        NodeKind::HtmlVoidElement,
        NodeKind::HtmlSelfClosingElement,
        NodeKind::HtmlRawNode,
    ]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(Check { local: BTreeSet::new(), used: Vec::new() }))
}

struct Check {
    local: BTreeSet<String>,
    used: Vec<UsedClass>,
}

/// `getAssetCSSClasses` and `getAllThemeCSSClasses`.
fn theme_css_classes<C>(cx: &Cx<'_, '_, C>) -> ThemeCssClasses {
    let mut asset = HashSet::new();
    if let Ok(files) = cx.fs().read_directory(&cx.to_uri("assets")) {
        for (uri, _) in files {
            if uri.ends_with(".css") {
                asset.extend(cx.css_classes_for_uri(&uri).iter().cloned());
            }
        }
    }
    let mut all = asset.clone();
    if let Ok(files) = recursive_read_directory(cx.fs(), &cx.to_uri(""), &|uri, _| uri.ends_with(".liquid")) {
        for uri in files {
            all.extend(cx.css_classes_for_uri(&uri).iter().cloned());
        }
    }
    ThemeCssClasses { asset, all }
}

impl<'a> LiquidCheck<'a> for Check {
    fn enter(&mut self, node: &'a Node<'a>, _: &[&'a Node<'a>], _cx: &LiquidCx<'a, '_>) {
        match node {
            Node::LiquidRawTag(tag) if tag.name == "stylesheet" => {
                if let Node::RawMarkup(body) = &tag.body {
                    self.local.extend(extract_css_class_names(body.value));
                }
            }
            Node::HtmlElement(n) => collect_used_classes(&n.attributes, &mut self.used),
            Node::HtmlVoidElement(n) => collect_used_classes(&n.attributes, &mut self.used),
            Node::HtmlSelfClosingElement(n) => collect_used_classes(&n.attributes, &mut self.used),
            Node::HtmlRawNode(n) if n.name == "svg" => {
                if let Node::RawMarkup(body) = &n.body {
                    collect_used_classes_from_svg(&n.attributes, body.value, body.position.start, &mut self.used);
                }
            }
            _ => {}
        }
    }

    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {
        if self.used.is_empty() || !cx.has_references() || !cx.has_dependencies() || !cx.has_css_classes() {
            return;
        }
        // Upstream unions every in-scope set for each file. Only classes defined somewhere
        // in the theme can be reported, so the graph is walked for those alone.
        let theme = cx.run.theme_css.get_or_init(|| theme_css_classes(cx));
        let mut candidates: Vec<&UsedClass> = self
            .used
            .iter()
            .filter(|u| theme.all.contains(&u.class_name) && !theme.asset.contains(&u.class_name) && !self.local.contains(&u.class_name))
            .collect();
        if candidates.is_empty() {
            return;
        }

        let get_references = |u: &str| cx.references(u);
        let get_dependencies = |u: &str| cx.dependencies(u);
        let ancestors = ancestor_uris(cx.uri(), &get_references);
        let mut rendered = rendered_snippet_uris(cx.uri(), &get_dependencies);
        for ancestor in &ancestors {
            rendered.extend(rendered_snippet_uris(ancestor, &get_dependencies));
        }
        let snippets = all_snippet_descendant_uris(&rendered, &get_dependencies);
        for uri in ancestors.iter().chain(&snippets) {
            let classes = cx.css_classes_for_uri(uri);
            candidates.retain(|u| !classes.contains(&u.class_name));
            if candidates.is_empty() {
                return;
            }
        }

        for u in candidates {
            cx.report(LiquidProblem::new(
                format!("CSS class '{}' may be defined outside the scope of this file.", u.class_name),
                u.start,
                u.end,
            ));
        }
    }
}
