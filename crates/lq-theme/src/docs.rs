//! The theme docset and JSON schemas, embedded in the binary.
//!
//! Filters, objects, tags and system translation keys are static tables generated
//! by `build.rs`. JSON Schemas stay text until the validator first needs one.
//! Fields checks don't read (descriptions, examples, ...) are not embedded; add
//! them in `build.rs` when a check needs one.

use lq_json::validator::SchemaDefinition;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::config::Mode;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parameter {
    pub name: &'static str,
    pub positional: bool,
    pub required: bool,
    pub types: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReturnType {
    /// `type`.
    pub ty: &'static str,
    pub name: &'static str,
    pub array_value: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterEntry {
    pub name: &'static str,
    pub deprecated: bool,
    pub deprecation_reason: Option<&'static str>,
    pub parameters: &'static [Parameter],
    pub return_type: &'static [ReturnType],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parent {
    pub object: &'static str,
    pub property: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Access {
    pub global: bool,
    pub parents: &'static [Parent],
    pub template: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectEntry {
    pub name: &'static str,
    pub deprecated: bool,
    pub deprecation_reason: Option<&'static str>,
    pub access: Option<Access>,
    pub return_type: &'static [ReturnType],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TagEntry {
    pub name: &'static str,
    pub deprecated: bool,
    pub deprecation_reason: Option<&'static str>,
    pub parameters: &'static [Parameter],
}

include!(concat!(env!("OUT_DIR"), "/docs_gen.rs"));

/// `ThemeDocset`: the async methods upstream has, as plain borrowed lists.
pub trait ThemeDocset: Send + Sync {
    /// Whether it was augmented prior to being passed.
    fn is_augmented(&self) -> bool {
        false
    }
    fn filters(&self) -> &[FilterEntry];
    fn objects(&self) -> &[ObjectEntry];
    fn liquid_drops(&self) -> &[ObjectEntry];
    fn tags(&self) -> &[TagEntry];
    /// `key in systemTranslations` for a flat translation key.
    fn has_system_translation(&self, key: &str) -> bool;
    /// `Object.keys(systemTranslations).length`.
    fn system_translation_count(&self) -> usize;
    /// `filters.find(f => f.name === name)`.
    fn filter(&self, name: &str) -> Option<&FilterEntry> {
        self.filters().iter().find(|e| e.name == name)
    }
    /// `objects.find(o => o.name === name)`.
    fn object(&self, name: &str) -> Option<&ObjectEntry> {
        self.objects().iter().find(|e| e.name == name)
    }
    /// `tags.find(t => t.name === name)`.
    fn tag(&self, name: &str) -> Option<&TagEntry> {
        self.tags().iter().find(|e| e.name == name)
    }
}

/// The docs snapshot at `data/docs`.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbeddedDocset;

impl ThemeDocset for EmbeddedDocset {
    fn filters(&self) -> &[FilterEntry] {
        FILTERS
    }
    fn objects(&self) -> &[ObjectEntry] {
        OBJECTS
    }
    fn liquid_drops(&self) -> &[ObjectEntry] {
        OBJECTS
    }
    fn tags(&self) -> &[TagEntry] {
        TAGS
    }
    fn has_system_translation(&self, key: &str) -> bool {
        SYSTEM_TRANSLATION_KEYS.binary_search(&key).is_ok()
    }
    fn system_translation_count(&self) -> usize {
        SYSTEM_TRANSLATION_KEYS.len()
    }
}

const ALIASED_FILTERS: [(&str, &str); 3] =
    [("camelcase", "camelize"), ("handle", "handleize"), ("t", "translate")];

const UNDOCUMENTED_FILTERS: [&str; 34] = [
    "_online_store_editor_live_setting",
    "addresses_url",
    "app_block_path?",
    "app_block_path_for",
    "app_extension_path?",
    "app_snippet_path?",
    "cancel_customer_order_link",
    "debug",
    "delete_customer_address_link",
    "dev_shop?",
    "distance_from",
    "edit_customer_address_link",
    "encode_url_component",
    "excerpt",
    "format_code",
    "global_block_type?",
    "h",
    "handle_from",
    "installments_pricing",
    "link_to_theme",
    "login_button",
    "login_url",
    "logout_url",
    "pad_spaces",
    "paragraphize",
    "recover_password_link",
    "recover_url",
    "register_url",
    "registration_uuid_from",
    "root_account_url",
    "sentence",
    "theme_url",
    "unit",
    "weight",
];

const UNDOCUMENTED_OBJECT_ENTRY_KEYS: [&str; 13] = [
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

const UNDOCUMENTED_OBJECTS: [&str; 2] = ["customer_address", "product_variant"];
const UNDOCUMENTED_TAGS: [&str; 4] = ["elsif", "ifchanged", "when", "schema"];
const STRING_RETURN: &[ReturnType] = &[ReturnType { ty: "string", name: "", array_value: "" }];

fn bare_filter(name: &'static str) -> FilterEntry {
    FilterEntry { name, deprecated: false, deprecation_reason: None, parameters: &[], return_type: &[] }
}

/// `AugmentedThemeDocset`. Lists are computed on first use and shared.
pub struct AugmentedThemeDocset<'d> {
    inner: &'d dyn ThemeDocset,
    filters: OnceLock<Vec<FilterEntry>>,
    objects: OnceLock<Vec<ObjectEntry>>,
    liquid_drops: OnceLock<Vec<ObjectEntry>>,
    tags: OnceLock<Vec<TagEntry>>,
    filter_index: OnceLock<HashMap<&'static str, usize>>,
    object_index: OnceLock<HashMap<&'static str, usize>>,
    tag_index: OnceLock<HashMap<&'static str, usize>>,
}

impl<'d> AugmentedThemeDocset<'d> {
    pub fn new(inner: &'d dyn ThemeDocset) -> Self {
        AugmentedThemeDocset {
            inner,
            filters: OnceLock::new(),
            objects: OnceLock::new(),
            liquid_drops: OnceLock::new(),
            tags: OnceLock::new(),
            filter_index: OnceLock::new(),
            object_index: OnceLock::new(),
            tag_index: OnceLock::new(),
        }
    }
}

impl ThemeDocset for AugmentedThemeDocset<'_> {
    fn is_augmented(&self) -> bool {
        true
    }

    fn filters(&self) -> &[FilterEntry] {
        self.filters.get_or_init(|| {
            let official = self.inner.filters();
            let mut out = official.to_vec();
            for (alias, base) in ALIASED_FILTERS {
                out.push(match official.iter().find(|e| e.name == base) {
                    Some(e) => FilterEntry { name: alias, ..*e },
                    None => bare_filter(alias),
                });
            }
            out.extend(UNDOCUMENTED_FILTERS.iter().map(|n| bare_filter(n)));
            out
        })
    }

    fn objects(&self) -> &[ObjectEntry] {
        self.objects.get_or_init(|| {
            let mut out = self.inner.objects().to_vec();
            out.extend(UNDOCUMENTED_OBJECTS.iter().map(|name| ObjectEntry {
                name,
                deprecated: false,
                deprecation_reason: None,
                access: None,
                return_type: &[],
            }));
            out.extend(UNDOCUMENTED_OBJECT_ENTRY_KEYS.iter().map(|name| ObjectEntry {
                name,
                deprecated: false,
                deprecation_reason: None,
                access: Some(Access { global: false, parents: &[], template: &[] }),
                return_type: STRING_RETURN,
            }));
            out
        })
    }

    fn liquid_drops(&self) -> &[ObjectEntry] {
        self.liquid_drops.get_or_init(|| {
            self.inner
                .objects()
                .iter()
                .filter(|o| match &o.access {
                    None => true,
                    Some(_) if o.deprecated => false,
                    Some(a) => !a.global || !a.parents.is_empty(),
                })
                .copied()
                .collect()
        })
    }

    fn tags(&self) -> &[TagEntry] {
        self.tags.get_or_init(|| {
            let mut out = self.inner.tags().to_vec();
            out.extend(UNDOCUMENTED_TAGS.iter().map(|name| TagEntry {
                name,
                deprecated: false,
                deprecation_reason: None,
                parameters: &[],
            }));
            out
        })
    }

    fn has_system_translation(&self, key: &str) -> bool {
        self.inner.has_system_translation(key)
    }

    fn system_translation_count(&self) -> usize {
        self.inner.system_translation_count()
    }

    /// `filters.find(f => f.name === name)`.
    fn filter(&self, name: &str) -> Option<&FilterEntry> {
        let list = self.filters();
        let index = self.filter_index.get_or_init(|| first_index(list.iter().map(|e| e.name)));
        index.get(name).map(|&i| &list[i])
    }

    /// `objects.find(o => o.name === name)`.
    fn object(&self, name: &str) -> Option<&ObjectEntry> {
        let list = self.objects();
        let index = self.object_index.get_or_init(|| first_index(list.iter().map(|e| e.name)));
        index.get(name).map(|&i| &list[i])
    }

    /// `tags.find(t => t.name === name)`.
    fn tag(&self, name: &str) -> Option<&TagEntry> {
        let list = self.tags();
        let index = self.tag_index.get_or_init(|| first_index(list.iter().map(|e| e.name)));
        index.get(name).map(|&i| &list[i])
    }
}

fn first_index(names: impl Iterator<Item = &'static str>) -> HashMap<&'static str, usize> {
    let mut map = HashMap::new();
    for (i, n) in names.enumerate() {
        map.entry(n).or_insert(i);
    }
    map
}

const SCHEMA_ROOT: &str = "https://raw.githubusercontent.com/Shopify/theme-liquid-docs/main/schemas";

macro_rules! schema_text {
    ($file:literal) => {
        include_str!(concat!("../../../data/docs/", $file))
    };
}

/// `ThemeLiquidDocsManager.schemas(mode)`, in manifest order.
pub fn schemas(mode: Mode) -> Vec<SchemaDefinition> {
    let def = |uri: &str, file_match: Option<&str>, schema: &str| SchemaDefinition {
        uri: format!("{SCHEMA_ROOT}/{uri}"),
        file_match: file_match.map(|m| vec![m.to_string()]),
        schema: schema.to_string(),
    };
    match mode {
        Mode::App => vec![def(
            "theme/translations.json",
            Some("locales/*.json"),
            schema_text!("translations.json"),
        )],
        Mode::Theme => vec![
            def("theme/translations.json", Some("locales/*.json"), schema_text!("translations.json")),
            def("theme/theme_block.json", Some("blocks/*.liquid"), schema_text!("theme_block.json")),
            def(
                "theme/theme_settings.json",
                Some("config/settings_schema.json"),
                schema_text!("theme_settings.json"),
            ),
            def("theme/section.json", Some("sections/*.liquid"), schema_text!("section.json")),
            def("theme/settings.json", None, schema_text!("settings.json")),
            def("theme/strict_settings.json", None, schema_text!("strict_settings.json")),
            def("theme/setting.json", None, schema_text!("setting.json")),
            def("theme/default_setting_values.json", None, schema_text!("default_setting_values.json")),
            def("theme/app_block_entry.json", None, schema_text!("app_block_entry.json")),
            def("theme/theme_block_entry.json", None, schema_text!("theme_block_entry.json")),
            def("theme/targetted_block_entry.json", None, schema_text!("targetted_block_entry.json")),
            def("theme/preset_blocks.json", None, schema_text!("preset_blocks.json")),
            def("theme/preset.json", None, schema_text!("preset.json")),
            def("theme/local_block_entry.json", None, schema_text!("local_block_entry.json")),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_populated() {
        assert_eq!(FILTERS.len(), 155);
        assert_eq!(OBJECTS.len(), 142);
        assert_eq!(TAGS.len(), 33);
        assert!(EmbeddedDocset.has_system_translation("shopify.sentence.words_connector"));
        assert!(!EmbeddedDocset.has_system_translation("nope"));
    }

    #[test]
    fn augmentation() {
        let d = AugmentedThemeDocset::new(&EmbeddedDocset);
        assert!(d.filter("t").is_some());
        assert!(d.filter("debug").is_some());
        assert!(d.object("content_for_footer").is_some());
        assert!(d.tag("schema").is_some());
        assert_eq!(d.filters().len(), 155 + 3 + 34);
    }

    #[test]
    fn schema_manifest_matches_the_docs() {
        let text = include_str!("../../../data/docs/manifest_theme.json");
        let manifest = lq_json::parse_json(text).unwrap();
        let list = manifest.get("schemas").unwrap().as_array().unwrap();
        let ours = schemas(Mode::Theme);
        assert_eq!(list.len(), ours.len());
        for (m, o) in list.iter().zip(&ours) {
            let uri = m.get("uri").unwrap().as_str().unwrap();
            assert_eq!(o.uri, format!("{SCHEMA_ROOT}/{uri}"));
            assert_eq!(o.file_match.is_some(), m.get("fileMatch").is_some());
        }
    }
}
