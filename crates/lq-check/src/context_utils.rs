//! `context-utils.ts`.

use crate::fs::{FileSystem, FileType};
use crate::source::{SourceCode, Theme};
use lq_json::jsonc::parse_json;
use lq_json::{uri, Object, Value};

fn default_locale_file(fs: &dyn FileSystem, root_uri: &str, postfix: &str) -> Result<Option<String>, String> {
    let files = fs.read_directory(&uri::join(root_uri, &["locales"]))?;
    Ok(files.into_iter().map(|(u, _)| u).find(|u| u.ends_with(postfix)))
}

/// `getDefaultLocale`.
pub fn default_locale(fs: &dyn FileSystem, root_uri: &str, postfix: &str) -> String {
    match default_locale_file(fs, root_uri, postfix) {
        Ok(Some(file)) => {
            let base = uri::basename(&file, None);
            base.split('.').next().unwrap_or("").to_string()
        }
        _ => "en".to_string(),
    }
}

fn is_locale_buffer(s: &SourceCode<'_>, postfix: &str) -> bool {
    s.kind() == crate::api::SourceCodeType::Json && s.uri.contains("locales") && s.uri.ends_with(postfix)
}

/// `getDefaultTranslations`. Prefers the theme's own buffer, then the file system.
/// `None` is `undefined`, which an empty file produces.
pub fn default_translations(fs: &dyn FileSystem, theme: &Theme<'_>, root_uri: &str, postfix: &str) -> Option<Value> {
    if let Some(buffer) = theme.iter().find(|s| is_locale_buffer(s, postfix)) {
        if let Some(v) = parse_json(buffer.source) {
            if v.is_truthy() {
                return Some(v);
            }
        }
    }
    let empty = || Some(Value::Object(Object::new()));
    match default_locale_file(fs, root_uri, postfix) {
        Ok(Some(file)) => match fs.read_file(&file) {
            Ok(text) => parse_json(&text),
            Err(_) => empty(),
        },
        _ => empty(),
    }
}

/// `recursiveReadDirectory`.
pub fn recursive_read_directory(
    fs: &dyn FileSystem,
    u: &str,
    filter: &dyn Fn(&str, FileType) -> bool,
) -> Result<Vec<String>, String> {
    const IGNORED_FOLDERS: [&str; 6] = [".git", "node_modules", "dist", "build", "tmp", "vendor"];
    let mut out = Vec::new();
    for (entry, file_type) in fs.read_directory(u)? {
        if file_type == FileType::Directory {
            if IGNORED_FOLDERS.iter().any(|f| entry.ends_with(f)) {
                continue;
            }
            out.extend(recursive_read_directory(fs, &entry, filter)?);
        } else if filter(&entry, file_type) {
            out.push(entry);
        }
    }
    Ok(out)
}

/// `MetafieldDefinition`.
#[derive(Clone, Debug, PartialEq)]
pub struct MetafieldDefinition {
    pub key: Value,
    pub name: Value,
    pub namespace: Value,
    pub description: Value,
    pub type_category: Value,
    pub type_name: Value,
}

/// `MetafieldDefinitionMap`, as `(category, definitions)` for every category.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetafieldDefinitionMap(pub Vec<(&'static str, Vec<MetafieldDefinition>)>);

impl MetafieldDefinitionMap {
    pub fn get(&self, category: &str) -> &[MetafieldDefinition] {
        self.0.iter().find(|(c, _)| *c == category).map_or(&[], |(_, d)| d)
    }
}

pub const FETCHED_METAFIELD_CATEGORIES: [&str; 12] = [
    "article", "blog", "collection", "company", "company_location", "location", "market", "order",
    "page", "product", "variant", "shop",
];

/// `makeGetMetafieldDefinitions(fs)(rootUri)`.
pub fn metafield_definitions(fs: &dyn FileSystem, root_uri: &str) -> MetafieldDefinitionMap {
    let mut map: Vec<(&'static str, Vec<MetafieldDefinition>)> =
        FETCHED_METAFIELD_CATEGORIES.iter().map(|c| (*c, Vec::new())).collect();
    let Ok(content) = fs.read_file(&uri::join(root_uri, &[".shopify", "metafields.json"])) else {
        return MetafieldDefinitionMap(map);
    };
    let Some(json) = parse_json(&content) else { return MetafieldDefinitionMap(map) };
    let field = |d: &Value, k: &str| d.get(k).cloned().unwrap_or(Value::Null);
    for (category, slot) in &mut map {
        let Some(list) = json.get(category).and_then(Value::as_array) else { continue };
        // A definition without a `type` object throws in the map; the group stays empty.
        if list.iter().any(|d| !matches!(d.get("type"), Some(Value::Object(_)))) {
            continue;
        }
        *slot = list
            .iter()
            .map(|d| {
                let ty = d.get("type").unwrap_or(&Value::Null);
                MetafieldDefinition {
                    key: field(d, "key"),
                    name: field(d, "name"),
                    namespace: field(d, "namespace"),
                    description: field(d, "description"),
                    type_category: field(ty, "category"),
                    type_name: field(ty, "name"),
                }
            })
            .collect();
    }
    MetafieldDefinitionMap(map)
}

/// `findRoot(curr, fileExists)` from `find-root.ts`.
pub fn find_root(curr: &str, file_exists: &dyn Fn(&str) -> bool) -> Option<String> {
    let at = |dir: &str, name: &str| file_exists(&uri::join(dir, &[name]));
    let mut dir = curr.to_string();
    loop {
        let parent = uri::dirname(&dir);
        let is_root = at(&dir, "shopify.extension.toml")
            || at(&dir, ".theme-check.yml")
            || (at(&dir, ".git") && at(&dir, "assets") && at(&dir, "snippets"))
            || (at(&dir, "assets")
                && at(&dir, "snippets")
                && !at(&parent, ".theme-check.yml")
                && !at(&uri::dirname(&parent), ".theme-check.yml"));
        if is_root {
            return Some(dir);
        }
        if parent == dir {
            return None;
        }
        dir = parent;
    }
}
