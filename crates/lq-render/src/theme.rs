//! Theme files, parsed once with the Liquid-only parser (HTML is plain text to Liquid, as it is
//! on Shopify's renderer).
//!
//! Sources and ASTs are leaked to `'static`: a profile run parses each file once and keeps it
//! until the process exits, and it saves threading a lifetime through every value.

use lq_syntax::Document;
use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub struct ThemeFile {
    pub path: Rc<str>,
    pub source: &'static str,
    pub line_starts: Rc<Vec<u32>>,
    doc: OnceCell<Result<&'static Document<'static>, String>>,
}

impl ThemeFile {
    pub fn new(path: &str, source: String) -> ThemeFile {
        let source: &'static str = Box::leak(source.into_boxed_str());
        let mut line_starts = vec![0u32];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        ThemeFile {
            path: Rc::from(path),
            source,
            line_starts: Rc::new(line_starts),
            doc: OnceCell::new(),
        }
    }

    /// The parsed document, or the parse error message.
    pub fn doc(&self) -> Result<&'static Document<'static>, &str> {
        self.doc
            .get_or_init(|| {
                let parsed = lq_syntax::parse_liquid_only(self.source)
                    .or_else(|_| lq_syntax::parse_tolerant_liquid_only(self.source));
                match parsed {
                    Ok(d) => Ok(&*Box::leak(Box::new(d))),
                    Err(e) => Err(e.message.to_string()),
                }
            })
            .as_ref()
            .map(|d| *d)
            .map_err(|e| e.as_str())
    }

    pub fn is_parsed(&self) -> bool {
        self.doc.get().is_some()
    }

    /// 1-based line and column of a byte offset.
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let i = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        (i as u32 + 1, offset - self.line_starts[i] + 1)
    }

    pub fn json(&self) -> Option<lq_json::Value> {
        lq_json::parse_json(self.source)
    }
}

#[derive(Default)]
pub struct Theme {
    pub root: PathBuf,
    pub files: BTreeMap<String, ThemeFile>,
}

const DIRS: &[&str] =
    &["layout", "templates", "sections", "snippets", "blocks", "config", "locales"];

impl Theme {
    /// Loads the Liquid and JSON files a render can touch. Assets aren't needed.
    pub fn load(root: &Path) -> std::io::Result<Theme> {
        let mut theme = Theme { root: root.to_path_buf(), files: BTreeMap::new() };
        for dir in DIRS {
            let d = root.join(dir);
            let Ok(entries) = std::fs::read_dir(&d) else { continue };
            let mut stack: Vec<(PathBuf, String)> =
                entries.flatten().map(|e| (e.path(), format!("{dir}/"))).collect();
            while let Some((path, prefix)) = stack.pop() {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                if path.is_dir() {
                    // templates/customers/*.json
                    if let Ok(sub) = std::fs::read_dir(&path) {
                        for e in sub.flatten() {
                            stack.push((e.path(), format!("{prefix}{name}/")));
                        }
                    }
                    continue;
                }
                if !(name.ends_with(".liquid") || name.ends_with(".json")) {
                    continue;
                }
                let source = std::fs::read_to_string(&path)?;
                let rel = format!("{prefix}{name}");
                theme.files.insert(rel.clone(), ThemeFile::new(&rel, source));
            }
        }
        if !theme.files.keys().any(|k| k.starts_with("layout/")) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} has no layout/ directory; pass a theme root with --path", root.display()),
            ));
        }
        Ok(theme)
    }

    pub fn from_files(files: Vec<(String, String)>) -> Theme {
        let mut theme = Theme::default();
        for (path, source) in files {
            theme.files.insert(path.clone(), ThemeFile::new(&path, source));
        }
        theme
    }

    pub fn get(&self, path: &str) -> Option<&ThemeFile> {
        self.files.get(path)
    }

    pub fn json(&self, path: &str) -> Option<lq_json::Value> {
        self.get(path).and_then(ThemeFile::json)
    }

    /// The trimmed source of a template's line, by Shopify's template name (`snippets/price`).
    pub fn line(&self, template: &str, line: u32) -> Option<String> {
        let f = self.get(&format!("{template}.liquid")).or_else(|| self.get(template))?;
        let text = f.source.lines().nth(line.checked_sub(1)? as usize)?;
        Some(text.trim().to_string())
    }

    /// The default locale file, `locales/*.default.json`.
    pub fn default_locale(&self) -> Option<lq_json::Value> {
        self.files
            .iter()
            .find(|(k, _)| k.starts_with("locales/") && k.ends_with(".default.json"))
            .and_then(|(_, f)| f.json())
    }
}
