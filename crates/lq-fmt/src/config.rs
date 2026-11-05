//! `resolveConfig`: finds the Prettier config for a file, applies `overrides`, and merges
//! `.editorconfig`. JS, TS and TOML configs cannot be evaluated here and are reported.

use crate::glob::{path_matches_globs, Glob};
use crate::options::*;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    pub message: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConfigError {}

fn config_error(message: String) -> ConfigError {
    ConfigError { message }
}

const CONFIG_FILES: [&str; 17] = [
    ".prettierrc",
    ".prettierrc.json",
    ".prettierrc.yml",
    ".prettierrc.yaml",
    ".prettierrc.json5",
    ".prettierrc.js",
    "prettier.config.js",
    ".prettierrc.ts",
    "prettier.config.ts",
    ".prettierrc.mjs",
    "prettier.config.mjs",
    ".prettierrc.mts",
    "prettier.config.mts",
    ".prettierrc.cjs",
    "prettier.config.cjs",
    ".prettierrc.cts",
    "prettier.config.cts",
];

/// A located config: options plus `overrides`.
#[derive(Clone, Debug)]
pub struct ResolvedConfig {
    pub file: PathBuf,
    pub config: Map<String, Value>,
}

/// JSON with comments, unquoted keys and the like, through the JSON port's parser.
fn parse_json5(text: &str) -> Option<Value> {
    use crate::json::parser::{parse, Ast, Id, Kind};
    fn convert(ast: &Ast<'_>, id: Id) -> Option<Value> {
        Some(match &ast.nodes[id as usize].kind {
            Kind::Root { node } => convert(ast, *node)?,
            Kind::Null => Value::Null,
            Kind::Boolean(v) => Value::Bool(*v == "true"),
            Kind::Str(raw) => {
                if raw.starts_with('"') {
                    serde_json::from_str(raw).ok()?
                } else {
                    let body = &raw[1..raw.len() - 1];
                    Value::String(body.replace("\\'", "'"))
                }
            }
            Kind::Numeric(raw) => serde_json::from_str(raw)
                .ok()
                .or_else(|| raw.parse::<f64>().ok().map(Value::from))?,
            Kind::Identifier(name) => Value::String(name.to_string()),
            Kind::Template { .. } | Kind::Unary { .. } => return None,
            Kind::Array { elements } => {
                let mut out = Vec::new();
                for e in elements {
                    out.push(convert(ast, (*e)?)?);
                }
                Value::Array(out)
            }
            Kind::Object { properties } => {
                let mut out = Map::new();
                for p in properties {
                    let Kind::Property { key, value } = &ast.nodes[*p as usize].kind else {
                        return None;
                    };
                    let k = match convert(ast, *key)? {
                        Value::String(s) => s,
                        v => v.to_string(),
                    };
                    out.insert(k, convert(ast, *value)?);
                }
                Value::Object(out)
            }
            Kind::Property { .. } => return None,
        })
    }
    let ast = parse(text).ok()?;
    convert(&ast, ast.root)
}

fn yaml_scalar(s: &str) -> Value {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        return Value::Array(
            inner.split(',').map(str::trim).filter(|x| !x.is_empty()).map(yaml_scalar).collect(),
        );
    }
    if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
    {
        return Value::String(s[1..s.len() - 1].to_string());
    }
    match s {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" | "~" => Value::Null,
        _ => s
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::String(s.to_string())),
    }
}

struct YamlLine<'a> {
    indent: usize,
    text: &'a str,
}

/// The block-style YAML subset configs use: maps, lists and scalars.
fn parse_yaml(text: &str) -> Option<Value> {
    let lines: Vec<YamlLine<'_>> = text
        .lines()
        .filter_map(|l| {
            let stripped = match l.find(" #") {
                Some(i) if !l[..i].contains(['"', '\'']) => &l[..i],
                _ => l,
            };
            let trimmed = stripped.trim_end();
            let body = trimmed.trim_start();
            if body.is_empty() || body.starts_with('#') || body == "---" {
                return None;
            }
            Some(YamlLine { indent: trimmed.len() - body.len(), text: body })
        })
        .collect();
    if lines.is_empty() {
        return Some(Value::Object(Map::new()));
    }
    let mut pos = 0;
    let v = parse_yaml_block(&lines, &mut pos, lines[0].indent)?;
    (pos == lines.len()).then_some(v)
}

fn parse_yaml_block(lines: &[YamlLine<'_>], pos: &mut usize, indent: usize) -> Option<Value> {
    if lines[*pos].text.starts_with("- ") || lines[*pos].text == "-" {
        let mut out = Vec::new();
        while *pos < lines.len() && lines[*pos].indent == indent && lines[*pos].text.starts_with('-') {
            let rest = lines[*pos].text[1..].trim_start();
            let item_indent = indent + (lines[*pos].text.len() - rest.len());
            if rest.is_empty() {
                *pos += 1;
                if *pos < lines.len() && lines[*pos].indent > indent {
                    let child = lines[*pos].indent;
                    out.push(parse_yaml_block(lines, pos, child)?);
                } else {
                    out.push(Value::Null);
                }
            } else if rest.contains(": ") || rest.ends_with(':') {
                let mut map = Map::new();
                parse_yaml_entry(rest, lines, pos, item_indent, &mut map)?;
                while *pos < lines.len() && lines[*pos].indent == item_indent {
                    let text = lines[*pos].text;
                    parse_yaml_entry(text, lines, pos, item_indent, &mut map)?;
                }
                out.push(Value::Object(map));
            } else {
                out.push(yaml_scalar(rest));
                *pos += 1;
            }
        }
        return Some(Value::Array(out));
    }
    let mut map = Map::new();
    while *pos < lines.len() && lines[*pos].indent == indent {
        let text = lines[*pos].text;
        parse_yaml_entry(text, lines, pos, indent, &mut map)?;
    }
    Some(Value::Object(map))
}

fn parse_yaml_entry(
    text: &str,
    lines: &[YamlLine<'_>],
    pos: &mut usize,
    indent: usize,
    map: &mut Map<String, Value>,
) -> Option<()> {
    let (key, value) = text.split_once(':')?;
    let key = key.trim().trim_matches(['"', '\'']).to_string();
    let value = value.trim();
    *pos += 1;
    if value.is_empty() {
        if *pos < lines.len()
            && (lines[*pos].indent > indent
                || (lines[*pos].indent == indent && lines[*pos].text.starts_with('-')))
        {
            let child = lines[*pos].indent;
            map.insert(key, parse_yaml_block(lines, pos, child)?);
        } else {
            map.insert(key, Value::Null);
        }
    } else {
        map.insert(key, yaml_scalar(value));
    }
    Some(())
}

fn load_config_file(file: &Path) -> Result<Option<Map<String, Value>>, ConfigError> {
    let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let text = std::fs::read_to_string(file)
        .map_err(|e| config_error(format!("Unable to read {}: {e}", file.display())))?;
    let parsed = if name == "package.json" {
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| config_error(format!("Invalid {}: {e}", file.display())))?;
        match v.get("prettier") {
            None => return Ok(None),
            Some(Value::Object(m)) => Value::Object(m.clone()),
            Some(_) => {
                return Err(config_error(format!(
                    "{}: a shared config (\"prettier\": string) is not supported",
                    file.display()
                )))
            }
        }
    } else if name == ".prettierrc.json" {
        parse_json5(&text)
            .ok_or_else(|| config_error(format!("Invalid JSON in {}", file.display())))?
    } else if name == ".prettierrc.json5" {
        parse_json5(&text)
            .ok_or_else(|| config_error(format!("Invalid JSON5 in {}", file.display())))?
    } else if name == ".prettierrc" || name.ends_with(".yml") || name.ends_with(".yaml") {
        parse_json5(&text)
            .or_else(|| parse_yaml(&text))
            .ok_or_else(|| config_error(format!("Unsupported YAML in {}", file.display())))?
    } else {
        return Err(config_error(format!(
            "{} needs a JavaScript runtime or TOML parser, which lq does not have",
            file.display()
        )));
    };
    match parsed {
        Value::Object(m) => Ok(Some(m)),
        Value::String(_) => Err(config_error(format!(
            "{}: a shared config (string) is not supported",
            file.display()
        ))),
        _ => Err(config_error(format!("Config is only allowed to be an object, in {}", file.display()))),
    }
}

/// `searchPrettierConfig`, with a per-directory cache.
#[derive(Default)]
pub struct ConfigSearch {
    by_dir: Mutex<HashMap<PathBuf, Option<ResolvedConfig>>>,
    editorconfig: Mutex<HashMap<PathBuf, Option<EditorConfigFile>>>,
    pub explicit_config: Option<PathBuf>,
    pub use_editorconfig: bool,
}

impl ConfigSearch {
    pub fn new() -> Self {
        ConfigSearch { use_editorconfig: true, ..Default::default() }
    }

    fn find_in_dir(&self, dir: &Path) -> Result<Option<ResolvedConfig>, ConfigError> {
        let mut candidates: Vec<PathBuf> = vec![dir.join("package.json")];
        let yaml = dir.join("package.yaml");
        if yaml.is_file() {
            candidates.push(yaml);
        }
        candidates.extend(CONFIG_FILES.iter().map(|n| dir.join(n)));
        candidates.push(dir.join(".prettierrc.toml"));
        for c in candidates {
            if !c.is_file() {
                continue;
            }
            let name = c.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if name == "package.yaml" {
                let text = std::fs::read_to_string(&c).unwrap_or_default();
                if let Some(Value::Object(m)) =
                    parse_yaml(&text).as_ref().and_then(|v| v.get("prettier"))
                {
                    return Ok(Some(ResolvedConfig { file: c, config: m.clone() }));
                }
                continue;
            }
            if let Some(config) = load_config_file(&c)? {
                return Ok(Some(ResolvedConfig { file: c, config }));
            }
        }
        Ok(None)
    }

    fn search(&self, start: &Path) -> Result<Option<ResolvedConfig>, ConfigError> {
        if let Some(file) = &self.explicit_config {
            return Ok(load_config_file(file)?.map(|config| ResolvedConfig { file: file.clone(), config }));
        }
        let mut dir = Some(start);
        let mut visited: Vec<PathBuf> = Vec::new();
        let mut found = None;
        while let Some(d) = dir {
            if let Some(cached) = self.by_dir.lock().unwrap().get(d) {
                found = cached.clone();
                break;
            }
            visited.push(d.to_path_buf());
            if let Some(c) = self.find_in_dir(d)? {
                found = Some(c);
                break;
            }
            dir = d.parent();
        }
        let mut cache = self.by_dir.lock().unwrap();
        for v in visited {
            cache.insert(v, found.clone());
        }
        Ok(found)
    }

    /// `resolveConfig(file, { editorconfig })` merged into `Options`.
    pub fn options_for(&self, file: &Path) -> Result<Options, ConfigError> {
        let dir = file.parent().unwrap_or(Path::new("/"));
        let mut options = Options::default();
        if self.use_editorconfig {
            self.apply_editorconfig(file, &mut options);
        }
        if let Some(found) = self.search(dir)? {
            let config_dir = found.file.parent().unwrap_or(Path::new("/"));
            apply_options(&found.config, &mut options, &found.file)?;
            if let Some(Value::Array(overrides)) = found.config.get("overrides") {
                let relative = pathdiff(file, config_dir);
                for o in overrides {
                    let to_list = |v: Option<&Value>| -> Vec<String> {
                        match v {
                            Some(Value::String(s)) => vec![s.clone()],
                            Some(Value::Array(a)) => {
                                a.iter().filter_map(|x| x.as_str().map(String::from)).collect()
                            }
                            _ => Vec::new(),
                        }
                    };
                    let files = to_list(o.get("files"));
                    let excluded = to_list(o.get("excludeFiles"));
                    if path_matches_globs(&relative, &files, &excluded) {
                        if let Some(Value::Object(m)) = o.get("options") {
                            apply_options(m, &mut options, &found.file)?;
                        }
                    }
                }
            }
        }
        Ok(options)
    }

    fn apply_editorconfig(&self, file: &Path, options: &mut Options) {
        let Some(start) = file.parent() else {
            return;
        };
        let root = find_project_root(start);
        let mut files = Vec::new();
        let mut dir = Some(start);
        while let Some(d) = dir {
            let cached = {
                let mut cache = self.editorconfig.lock().unwrap();
                cache
                    .entry(d.to_path_buf())
                    .or_insert_with(|| {
                        std::fs::read_to_string(d.join(".editorconfig"))
                            .ok()
                            .map(|t| EditorConfigFile::parse(d, &t))
                    })
                    .clone()
            };
            if let Some(c) = cached {
                let stop = c.root;
                files.push(c);
                if stop {
                    break;
                }
            }
            if root.as_deref() == Some(d) {
                break;
            }
            dir = d.parent();
        }
        let mut merged: HashMap<String, String> = HashMap::new();
        for f in files.iter().rev() {
            f.apply(file, &mut merged);
        }
        apply_editorconfig_values(&merged, options);
    }
}

/// `findProjectRoot`: the nearest directory holding `.git` or a `.hg` directory.
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join(".git").exists() || d.join(".hg").is_dir() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

#[derive(Clone, Debug)]
struct EditorConfigFile {
    dir: PathBuf,
    root: bool,
    sections: Vec<(String, Vec<(String, String)>)>,
}

impl EditorConfigFile {
    fn parse(dir: &Path, text: &str) -> Self {
        let mut root = false;
        let mut sections: Vec<(String, Vec<(String, String)>)> = Vec::new();
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') || l.starts_with(';') {
                continue;
            }
            if l.starts_with('[') && l.ends_with(']') {
                sections.push((l[1..l.len() - 1].to_string(), Vec::new()));
                continue;
            }
            let Some((k, v)) = l.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_ascii_lowercase());
            match sections.last_mut() {
                Some(s) => s.1.push((k, v)),
                None if k == "root" => root = v == "true",
                None => {}
            }
        }
        EditorConfigFile { dir: dir.to_path_buf(), root, sections }
    }

    fn apply(&self, file: &Path, out: &mut HashMap<String, String>) {
        let relative = pathdiff(file, &self.dir);
        for (glob, values) in &self.sections {
            // editorconfig: no slash matches at any depth.
            let pattern = if glob.contains('/') {
                glob.trim_start_matches('/').to_string()
            } else {
                format!("**/{glob}")
            };
            if Glob::new(&pattern).is_match(&relative) {
                for (k, v) in values {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
    }
}

fn apply_editorconfig_values(values: &HashMap<String, String>, options: &mut Options) {
    let get = |k: &str| values.get(k).map(String::as_str);
    let positive = |v: Option<&str>| v.and_then(|s| s.parse::<usize>().ok()).filter(|n| *n > 0);
    let mut use_tabs = None;
    match get("indent_style") {
        Some("space") => use_tabs = Some(false),
        Some("tab") => use_tabs = Some(true),
        _ => {
            if get("indent_size") == Some("tab") {
                use_tabs = Some(true);
            }
        }
    }
    if let Some(t) = use_tabs {
        options.use_tabs = t;
    }
    // `indent_size` defaults `tab_width` in the editorconfig parser's post-processing.
    let tab_width = positive(get("tab_width"));
    let indent_size = positive(get("indent_size"));
    if use_tabs == Some(false) && indent_size.is_some() {
        options.tab_width = indent_size.unwrap_or(2);
    } else if let Some(w) = tab_width.or(indent_size.filter(|_| use_tabs.is_none())) {
        options.tab_width = w;
    }
    match get("max_line_length") {
        Some("off") => options.print_width = usize::MAX / 4,
        v => {
            if let Some(w) = positive(v) {
                options.print_width = w;
            }
        }
    }
    match get("quote_type") {
        Some("single") => options.single_quote = true,
        Some("double") => options.single_quote = false,
        _ => {}
    }
    match get("end_of_line") {
        Some("lf") => options.end_of_line = EndOfLine::Lf,
        Some("crlf") => options.end_of_line = EndOfLine::Crlf,
        Some("cr") => options.end_of_line = EndOfLine::Cr,
        _ => {}
    }
}

/// `path.relative(base, file)` with forward slashes, for paths under or beside `base`.
pub fn pathdiff(file: &Path, base: &Path) -> String {
    let f: Vec<_> = file.components().collect();
    let b: Vec<_> = base.components().collect();
    let common = f.iter().zip(b.iter()).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..b.len() {
        parts.push("..".to_string());
    }
    for c in &f[common..] {
        parts.push(c.as_os_str().to_string_lossy().to_string());
    }
    parts.join("/")
}

fn invalid(name: &str, value: &Value, file: &Path) -> ConfigError {
    config_error(format!("Invalid {name} value in {}: {value}", file.display()))
}

/// Applies Prettier option names onto `Options`. Unknown keys are ignored, as Prettier
/// warns and continues.
pub fn apply_options(
    map: &Map<String, Value>,
    options: &mut Options,
    file: &Path,
) -> Result<(), ConfigError> {
    for (k, v) in map {
        let as_bool = || v.as_bool().ok_or_else(|| invalid(k, v, file));
        let as_usize = || v.as_u64().map(|n| n as usize).ok_or_else(|| invalid(k, v, file));
        let as_str = || v.as_str().ok_or_else(|| invalid(k, v, file));
        match k.as_str() {
            "printWidth" => options.print_width = as_usize()?,
            "tabWidth" => options.tab_width = as_usize()?,
            "useTabs" => options.use_tabs = as_bool()?,
            "singleQuote" => options.single_quote = as_bool()?,
            "bracketSameLine" => options.bracket_same_line = as_bool()?,
            "singleAttributePerLine" => options.single_attribute_per_line = as_bool()?,
            "bracketSpacing" => options.bracket_spacing = as_bool()?,
            "semi" => options.semi = as_bool()?,
            "jsxSingleQuote" => options.jsx_single_quote = as_bool()?,
            "liquidSingleQuote" => options.liquid_single_quote = as_bool()?,
            "embeddedSingleQuote" => options.embedded_single_quote = as_bool()?,
            "singleLineLinkTags" => options.single_line_link_tags = as_bool()?,
            "indentSchema" => options.indent_schema = as_bool()?,
            "liquidDocParamDash" => options.liquid_doc_param_dash = as_bool()?,
            "htmlWhitespaceSensitivity" => {
                options.html_whitespace_sensitivity = match as_str()? {
                    "css" => HtmlWhitespaceSensitivity::Css,
                    "strict" => HtmlWhitespaceSensitivity::Strict,
                    "ignore" => HtmlWhitespaceSensitivity::Ignore,
                    _ => return Err(invalid(k, v, file)),
                }
            }
            "captureWhitespaceSensitivity" => {
                options.capture_whitespace_sensitivity = match as_str()? {
                    "strict" => CaptureWhitespaceSensitivity::Strict,
                    "ignore" => CaptureWhitespaceSensitivity::Ignore,
                    _ => return Err(invalid(k, v, file)),
                }
            }
            "endOfLine" => {
                options.end_of_line = match as_str()? {
                    "lf" => EndOfLine::Lf,
                    "crlf" => EndOfLine::Crlf,
                    "cr" => EndOfLine::Cr,
                    "auto" => EndOfLine::Auto,
                    _ => return Err(invalid(k, v, file)),
                }
            }
            "embeddedLanguageFormatting" => {
                options.embedded_language_formatting = match as_str()? {
                    "auto" => EmbeddedLanguageFormatting::Auto,
                    "off" => EmbeddedLanguageFormatting::Off,
                    _ => return Err(invalid(k, v, file)),
                }
            }
            "objectWrap" => {
                options.object_wrap = match as_str()? {
                    "preserve" => ObjectWrap::Preserve,
                    "collapse" => ObjectWrap::Collapse,
                    _ => return Err(invalid(k, v, file)),
                }
            }
            "trailingComma" => {
                options.trailing_comma = match as_str()? {
                    "all" => TrailingComma::All,
                    "es5" => TrailingComma::Es5,
                    "none" => TrailingComma::None,
                    _ => return Err(invalid(k, v, file)),
                }
            }
            "quoteProps" => {
                options.quote_props = match as_str()? {
                    "as-needed" => QuoteProps::AsNeeded,
                    "consistent" => QuoteProps::Consistent,
                    "preserve" => QuoteProps::Preserve,
                    _ => return Err(invalid(k, v, file)),
                }
            }
            "arrowParens" => {
                options.arrow_parens = match as_str()? {
                    "always" => ArrowParens::Always,
                    "avoid" => ArrowParens::Avoid,
                    _ => return Err(invalid(k, v, file)),
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// One-off `resolveConfig` for a file.
pub fn resolve_options(file: &Path) -> Result<Options, ConfigError> {
    ConfigSearch::new().options_for(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_subset() {
        let v = parse_yaml(
            "printWidth: 100\nplugins:\n  - a\noverrides:\n  - files: \"*.liquid\"\n    options:\n      singleQuote: false\n",
        )
        .unwrap();
        assert_eq!(v["printWidth"], 100);
        assert_eq!(v["plugins"][0], "a");
        assert_eq!(v["overrides"][0]["files"], "*.liquid");
        assert_eq!(v["overrides"][0]["options"]["singleQuote"], false);
    }

    #[test]
    fn json5_config() {
        let v = parse_json5("{ // c\n printWidth: 100, 'a': 'b', }").unwrap();
        assert_eq!(v["printWidth"], 100);
        assert_eq!(v["a"], "b");
    }

    #[test]
    fn relative_paths() {
        assert_eq!(pathdiff(Path::new("/a/b/c"), Path::new("/a")), "b/c");
        assert_eq!(pathdiff(Path::new("/a/c"), Path::new("/a/b")), "../c");
    }
}
