//! `Dependencies`: what a run gets from its environment. Lazily computed, shared.

use crate::fs::{FileSystem, NodeFileSystem};
use crate::graph::{build_theme_graph, Reference, ThemeGraph};
use crate::liquid_doc::{extract_doc_definition, file_path_supports_liquid_doc, DocDefinition};
use crate::source::Theme;
use crate::to_schema::{is_block, is_section, to_schema, IsValidSchema, ThemeSchema};
use crate::utils::styles::{css_classes_from_asset_uri, css_classes_from_liquid_uri};
use lq_json::uri;
use lq_json::validator::JsonValidator;
use lq_theme::config::Config;
use lq_theme::docs::{EmbeddedDocset, ThemeDocset};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, OnceLock};

/// The sync form of upstream's `Dependencies`. Optional dependencies are `None`/empty
/// when absent; `has_*` distinguishes absent where checks care.
pub trait Dependencies<'a>: Send + Sync {
    fn fs(&'a self) -> &'a dyn FileSystem;
    fn theme_docset(&'a self) -> Option<&'a dyn ThemeDocset>;
    fn has_json_validator(&self) -> bool;
    fn json_validator(&'a self) -> Option<&'a JsonValidator>;
    fn block_schema(&'a self, name: &str) -> Option<&'a ThemeSchema<'a>>;
    fn section_schema(&'a self, name: &str) -> Option<&'a ThemeSchema<'a>>;
    fn app_block_schema(&'a self, name: &str) -> Option<&'a ThemeSchema<'a>>;
    fn doc_definition(&'a self, relative_path: &str) -> Option<&'a DocDefinition>;
    fn has_css_classes(&self) -> bool;
    fn css_classes_for_uri(&'a self, uri: &str) -> Arc<BTreeSet<String>>;
    fn has_references(&self) -> bool;
    /// Why the theme graph failed to build, if it did.
    fn graph_error(&'a self) -> Option<&'a str> {
        None
    }
    fn references(&'a self, uri: &str) -> Vec<Reference>;
    fn has_dependencies(&self) -> bool;
    fn dependencies(&'a self, uri: &str) -> Vec<Reference>;
    /// `None` means use the default, read from `.shopify/metafields.json`.
    fn metafield_definitions(&'a self, _root_uri: &str) -> Option<crate::context_utils::MetafieldDefinitionMap> {
        None
    }
}

struct Lazy<T> {
    index: usize,
    cell: OnceLock<T>,
}

impl<T> Lazy<T> {
    fn new(index: usize) -> Self {
        Lazy { index, cell: OnceLock::new() }
    }
}

/// The dependencies `themeCheckRun` wires up: real files, embedded docs, no network.
pub struct NodeDependencies<'a> {
    theme: &'a Theme<'a>,
    config: &'a Config,
    fs: NodeFileSystem,
    docset: EmbeddedDocset,
    validator: OnceLock<JsonValidator>,
    block_schemas: HashMap<String, Lazy<Option<ThemeSchema<'a>>>>,
    section_schemas: HashMap<String, Lazy<Option<ThemeSchema<'a>>>>,
    doc_definitions: HashMap<String, Lazy<Option<DocDefinition>>>,
    liquid_uris: HashMap<&'a str, OnceLock<Arc<BTreeSet<String>>>>,
    css_assets: OnceLock<HashMap<String, OnceLock<Arc<BTreeSet<String>>>>>,
    graph: OnceLock<Result<ThemeGraph, String>>,
}

impl<'a> NodeDependencies<'a> {
    /// `root` is the theme path given to `themeCheckRun`.
    pub fn new(theme: &'a Theme<'a>, config: &'a Config, root: &str) -> Self {
        let mut block_schemas = HashMap::new();
        let mut section_schemas = HashMap::new();
        let mut doc_definitions = HashMap::new();
        let mut liquid_uris = HashMap::new();
        let encoded_root = uri::Uri::file(root).to_string_with(false);
        for (i, file) in theme.iter().enumerate() {
            let is_liquid = file.uri.ends_with(".liquid");
            if is_block(&file.uri) {
                block_schemas.insert(uri::basename(&file.uri, Some(".liquid")), Lazy::new(i));
            }
            if is_section(&file.uri) {
                section_schemas.insert(uri::basename(&file.uri, Some(".liquid")), Lazy::new(i));
            }
            doc_definitions.insert(uri::relative(&file.uri, &encoded_root), Lazy::new(i));
            if is_liquid {
                liquid_uris.insert(file.uri.as_str(), OnceLock::new());
            }
        }
        NodeDependencies {
            theme,
            config,
            fs: NodeFileSystem,
            docset: EmbeddedDocset,
            validator: OnceLock::new(),
            block_schemas,
            section_schemas,
            doc_definitions,
            liquid_uris,
            css_assets: OnceLock::new(),
            graph: OnceLock::new(),
        }
    }

    fn validator(&'a self) -> &'a JsonValidator {
        self.validator
            .get_or_init(|| JsonValidator::new(lq_theme::docs::schemas(self.config.context)))
    }

    fn schema_from(&'a self, lazy: &'a Lazy<Option<ThemeSchema<'a>>>) -> Option<&'a ThemeSchema<'a>> {
        lazy.cell
            .get_or_init(|| {
                let file = &self.theme[lazy.index];
                let crate::source::Ast::Liquid(_) = &file.ast else { return None };
                to_schema(
                    self.config.context,
                    &file.uri,
                    file.liquid_ast(),
                    IsValidSchema::Validator(self.validator()),
                )
            })
            .as_ref()
    }

    fn graph_result(&'a self) -> &'a Result<ThemeGraph, String> {
        self.graph.get_or_init(|| {
            build_theme_graph(&self.config.root_uri, self, self.theme, self.config.context, self.config.divergences)
        })
    }

    /// Why the graph failed, if it was built during the run and failed.
    pub fn graph_failure(&self) -> Option<&str> {
        self.graph.get().and_then(|g| g.as_ref().err()).map(String::as_str)
    }

    fn graph(&'a self) -> Option<&'a ThemeGraph> {
        self.graph_result().as_ref().ok()
    }

    fn css_assets(&self) -> &HashMap<String, OnceLock<Arc<BTreeSet<String>>>> {
        self.css_assets.get_or_init(|| {
            let mut map = HashMap::new();
            if let Ok(files) = self.fs.read_directory(&uri::join(&self.config.root_uri, &["assets"])) {
                for (u, _) in files {
                    if u.ends_with(".css") {
                        map.insert(u, OnceLock::new());
                    }
                }
            }
            map
        })
    }
}

impl<'a> Dependencies<'a> for NodeDependencies<'a> {
    fn fs(&'a self) -> &'a dyn FileSystem {
        &self.fs
    }

    fn theme_docset(&'a self) -> Option<&'a dyn ThemeDocset> {
        Some(&self.docset)
    }

    fn has_json_validator(&self) -> bool {
        true
    }

    fn json_validator(&'a self) -> Option<&'a JsonValidator> {
        Some(self.validator())
    }

    fn block_schema(&'a self, name: &str) -> Option<&'a ThemeSchema<'a>> {
        self.schema_from(self.block_schemas.get(name)?)
    }

    fn section_schema(&'a self, name: &str) -> Option<&'a ThemeSchema<'a>> {
        self.schema_from(self.section_schemas.get(name)?)
    }

    fn app_block_schema(&'a self, name: &str) -> Option<&'a ThemeSchema<'a>> {
        self.block_schema(name)
    }

    fn doc_definition(&'a self, relative_path: &str) -> Option<&'a DocDefinition> {
        let lazy = self.doc_definitions.get(relative_path)?;
        lazy.cell
            .get_or_init(|| {
                let file = &self.theme[lazy.index];
                let crate::source::Ast::Liquid(Ok(doc)) = &file.ast else { return None };
                if !file_path_supports_liquid_doc(&file.uri) {
                    return None;
                }
                Some(extract_doc_definition(&file.uri, &doc.root))
            })
            .as_ref()
    }

    fn has_css_classes(&self) -> bool {
        true
    }

    fn css_classes_for_uri(&'a self, u: &str) -> Arc<BTreeSet<String>> {
        if let Some(cell) = self.css_assets().get(u) {
            return cell.get_or_init(|| Arc::new(css_classes_from_asset_uri(u, &self.fs))).clone();
        }
        match self.liquid_uris.get(u) {
            Some(cell) => cell.get_or_init(|| Arc::new(css_classes_from_liquid_uri(u, &self.fs))).clone(),
            None => Arc::new(BTreeSet::new()),
        }
    }

    fn has_references(&self) -> bool {
        true
    }

    fn graph_error(&'a self) -> Option<&'a str> {
        self.graph_result().as_ref().err().map(String::as_str)
    }

    fn references(&'a self, u: &str) -> Vec<Reference> {
        self.graph().map_or_else(Vec::new, |g| g.references(u).to_vec())
    }

    fn has_dependencies(&self) -> bool {
        true
    }

    fn dependencies(&'a self, u: &str) -> Vec<Reference> {
        self.graph().map_or_else(Vec::new, |g| g.dependencies(u).to_vec())
    }
}
