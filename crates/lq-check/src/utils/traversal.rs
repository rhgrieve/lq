//! `utils/traversal.ts`.

use crate::graph::{Reference, ReferenceType};
use crate::to_schema::is_snippet;
use std::collections::{HashSet, VecDeque};

/// `getAncestorUris(uri, getReferences)`.
pub fn ancestor_uris(uri: &str, get_references: &dyn Fn(&str) -> Vec<Reference>) -> Vec<String> {
    let mut ancestors = Vec::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::from([uri.to_string()]);
    while let Some(current) = queue.pop_front() {
        if !visited.insert(current.clone()) {
            continue;
        }
        for r in get_references(&current) {
            if r.ty == ReferenceType::Direct && !visited.contains(&r.source.uri) {
                ancestors.push(r.source.uri.clone());
                queue.push_back(r.source.uri);
            }
        }
    }
    ancestors
}

/// `getAllSnippetDescendantUris(initialUris, getDependencies)`.
pub fn all_snippet_descendant_uris(initial: &[String], get_dependencies: &dyn Fn(&str) -> Vec<Reference>) -> Vec<String> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    let mut queue: VecDeque<String> = initial.iter().cloned().collect();
    while let Some(current) = queue.pop_front() {
        if !visited.insert(current.clone()) {
            continue;
        }
        result.push(current.clone());
        for dep in get_dependencies(&current) {
            if dep.ty == ReferenceType::Direct && is_snippet(&dep.target.uri) && !visited.contains(&dep.target.uri) {
                queue.push_back(dep.target.uri);
            }
        }
    }
    result
}

/// `getRenderedSnippetUris(uri, getDependencies)`.
pub fn rendered_snippet_uris(uri: &str, get_dependencies: &dyn Fn(&str) -> Vec<Reference>) -> Vec<String> {
    get_dependencies(uri)
        .into_iter()
        .filter(|d| d.ty == ReferenceType::Direct && is_snippet(&d.target.uri))
        .map(|d| d.target.uri)
        .collect()
}
