//! A port of theme-check-node's `src/config`: reading YAML config files, merging
//! `extends` chains, loading a description into a [`Config`] and validating it.
//!
//! Deviations from upstream:
//! - Third-party checks (`require` entries and auto-discovered `theme-check-*`
//!   packages) cannot load in Rust. They are resolved as upstream does, kept in
//!   [`Config::require`], and otherwise only produce a note on stderr.
//! - When several `extends` of one file fail, upstream reports whichever promise
//!   rejects first; here it is the first in list order.
//! - `severity` set to an `Object.prototype` member name (`toString`) is a function
//!   upstream and fails validation with `got function`. Here it is an unsupported
//!   severity error.
//! - A top-level config key or setting called `__proto__` becomes an own key here
//!   rather than setting a prototype.
//! - Node's resolution from `extends`/`require` covers `node_modules` lookup up the
//!   tree, `main`, `exports` (strings, subpaths, conditions `node`, `require`,
//!   `module-sync`, `default`, trailing `*` patterns) and `index.*`. Not covered:
//!   builtin module names, `NODE_PATH`, global folders and package self-reference.
//! - A non-string `ignore` entry makes upstream throw a `TypeError` from `isIgnored`
//!   when the theme is listed. See [`Config::ignore_type_error`].

use crate::ignore::rewrite_pattern;
use crate::minimatch::Minimatch;
use crate::yaml;
use lq_json::uri;
use lq_json::value::{Object, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaType {
    String,
    Number,
    Boolean,
    Object,
    Array,
}

/// A setting's default, const-constructible for a static registry table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DefaultValue {
    Bool(bool),
    Num(f64),
    Str(&'static str),
    StrArray(&'static [&'static str]),
    EmptyArray,
    Null,
}

impl DefaultValue {
    pub fn to_value(self) -> Value {
        match self {
            DefaultValue::Bool(b) => Value::Bool(b),
            DefaultValue::Num(n) => Value::Number(n),
            DefaultValue::Str(s) => Value::String(s.to_string()),
            DefaultValue::StrArray(a) => Value::Array(a.iter().map(|s| Value::String((*s).to_string())).collect()),
            DefaultValue::EmptyArray => Value::Array(Vec::new()),
            DefaultValue::Null => Value::Null,
        }
    }

    /// `value === defaultValue`: arrays are never identical to a config value.
    fn identical_to(self, v: &Value) -> bool {
        match (self, v) {
            (DefaultValue::Bool(a), Value::Bool(b)) => a == *b,
            (DefaultValue::Num(a), Value::Number(b)) => a == *b,
            (DefaultValue::Str(a), Value::String(b)) => a == b,
            (DefaultValue::Null, Value::Null) => true,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SchemaProp {
    pub ty: SchemaType,
    pub default: Option<DefaultValue>,
    pub optional: bool,
    pub properties: &'static [(&'static str, SchemaProp)],
    pub item_type: Option<&'static SchemaProp>,
}

#[derive(Clone, Copy, Debug)]
pub struct CheckInfo {
    pub code: &'static str,
    pub aliases: &'static [&'static str],
    pub schema: &'static [(&'static str, SchemaProp)],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Theme,
    App,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Theme => "theme",
            Mode::App => "app",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    pub name: String,
    pub message: String,
}

impl ConfigError {
    fn new(name: &str, message: impl Into<String>) -> ConfigError {
        ConfigError { name: name.to_string(), message: message.into() }
    }
    fn error(message: impl Into<String>) -> ConfigError {
        ConfigError::new("Error", message)
    }
    fn type_error(message: impl Into<String>) -> ConfigError {
        ConfigError::new("TypeError", message)
    }
    fn assertion(message: impl Into<String>) -> ConfigError {
        ConfigError::new("AssertionError", message)
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug)]
pub struct Config {
    pub context: Mode,
    /// Check code to settings (camelCased keys, numeric severity), in JS key order.
    /// May contain the literal `undefined` key; see `unaliased_settings`.
    pub settings: Object,
    /// Enabled check codes, in registry order.
    pub checks: Vec<&'static str>,
    pub root_uri: String,
    /// `None` only from [`Config::new_for_test`]; a loaded config always has a list.
    pub ignore: Option<Vec<String>>,
    /// Resolved `require` paths: kept, but nothing is loaded from them.
    pub require: Vec<String>,
    /// Which fixes for upstream bugs are on. `--upstream-bugs` empties it.
    pub divergences: crate::divergence::Divergences,
    ignore_values: Option<Vec<Value>>,
    global_matchers: OnceLock<Vec<Minimatch>>,
    check_matchers: OnceLock<HashMap<String, Vec<Minimatch>>>,
}

impl Config {
    /// `config.settings[code]`, if it is an object.
    pub fn check_settings(&self, code: &str) -> Option<&Object> {
        self.settings.get(code).and_then(Value::as_object)
    }

    pub fn new_for_test(
        context: Mode,
        settings: Object,
        checks: Vec<&'static str>,
        root_uri: String,
        ignore: Option<Vec<String>>,
    ) -> Config {
        let ignore_values = ignore.as_ref().map(|i| i.iter().map(|s| Value::String(s.clone())).collect());
        Config {
            context,
            settings,
            checks,
            root_uri,
            ignore,
            require: Vec::new(),
            divergences: crate::divergence::Divergences::ALL,
            ignore_values,
            global_matchers: OnceLock::new(),
            check_matchers: OnceLock::new(),
        }
    }

    /// The `ignore` list as written, non-strings included (`ignore` keeps strings only).
    pub fn ignore_values(&self) -> Option<&[Value]> {
        self.ignore_values.as_deref()
    }

    /// What upstream's `isIgnored` throws once it is asked about any file, when the
    /// global `ignore` list holds a non-string. Listing a theme fails with it.
    pub fn ignore_type_error(&self) -> Option<ConfigError> {
        let values = self.ignore_values.as_ref()?;
        let bad = values.iter().find(|v| !matches!(v, Value::String(_)))?;
        Some(match bad {
            Value::Null => ConfigError::type_error("Cannot read properties of null (reading 'replace')"),
            _ => ConfigError::type_error("pattern.replace is not a function"),
        })
    }

    pub(crate) fn global_ignore_matchers(&self) -> &[Minimatch] {
        self.global_matchers.get_or_init(|| {
            self.ignore
                .iter()
                .flatten()
                .map(|p| Minimatch::new(&rewrite_pattern(p, &self.root_uri)))
                .collect()
        })
    }

    pub(crate) fn check_ignore_matchers(&self, code: &str) -> Option<&[Minimatch]> {
        let map = self.check_matchers.get_or_init(|| {
            let mut map = HashMap::new();
            for (code, settings) in self.settings.iter() {
                let list = settings.get("ignore").and_then(Value::as_array).unwrap_or(&[]);
                if list.is_empty() {
                    continue;
                }
                let matchers = list
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|p| Minimatch::new(&rewrite_pattern(p, &self.root_uri)))
                    .collect();
                map.insert(code.to_string(), matchers);
            }
            map
        });
        map.get(code).map(Vec::as_slice)
    }
}

// ---------------------------------------------------------------------------
// node:path and node:fs error shapes
// ---------------------------------------------------------------------------

fn is_absolute(p: &str) -> bool {
    p.starts_with('/')
}

/// `path.resolve(root, p)`, where `root` is absolute.
fn path_resolve(root: &str, p: &str) -> String {
    let joined = if is_absolute(p) { p.to_string() } else { format!("{root}/{p}") };
    let n = uri::posix_normalize(&joined);
    if n.len() > 1 {
        n.trim_end_matches('/').to_string()
    } else {
        n
    }
}

fn path_join(a: &str, b: &str) -> String {
    uri::posix_join(&[a, b])
}

fn fs_error(e: &std::io::Error, syscall: &str, path: &str) -> ConfigError {
    use std::io::ErrorKind as K;
    let with_path = |code: &str, text: &str| format!("{code}: {text}, {syscall} '{path}'");
    let raw = e.raw_os_error();
    let message = match (e.kind(), raw) {
        (K::NotFound, _) => with_path("ENOENT", "no such file or directory"),
        (K::PermissionDenied, _) => with_path("EACCES", "permission denied"),
        (_, Some(20)) => with_path("ENOTDIR", "not a directory"),
        (_, Some(62)) => with_path("ELOOP", "too many symbolic links encountered"),
        (_, Some(21)) => "EISDIR: illegal operation on a directory, read".to_string(),
        _ => format!("{e}, {syscall} '{path}'"),
    };
    ConfigError::error(message)
}

// ---------------------------------------------------------------------------
// read-yaml.ts
// ---------------------------------------------------------------------------

/// `${value}` in a template literal.
fn js_string(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => lq_json::js::number_to_string(*n),
        Value::String(s) => s.clone(),
        Value::Object(_) => "[object Object]".into(),
        Value::Array(a) => a
            .iter()
            .map(|x| if x.is_null() { String::new() } else { js_string(x) })
            .collect::<Vec<_>>()
            .join(","),
    }
}

fn parse_yaml_file(absolute_path: &str, contents: &str) -> Result<Object, ConfigError> {
    let result = if contents.trim_matches(lq_json::js::is_js_whitespace).is_empty() {
        Value::Object(Object::new())
    } else {
        yaml::parse(contents).map_err(|e| {
            if e.name == "ReferenceError" && e.message.contains("Unresolved alias") {
                if let Some(i) = e.message.find(": ") {
                    let rest = &e.message[i + 2..];
                    let alias = rest.split('\n').next().unwrap_or(rest);
                    return ConfigError::new(
                        "UnresolvedAliasError",
                        format!(
                            "YAML parsing error: Unresolved alias *{alias}.\nDid you forget to wrap your ignore statement in quotes? '*{alias}'\n{}",
                            e.message
                        ),
                    );
                }
            }
            ConfigError::new(&e.name, e.message)
        })?
    };
    match result {
        Value::Object(o) => Ok(o),
        _ => Err(ConfigError::error(format!(
            "Expecting parsed contents of config file at path '{absolute_path}' to be a plain object"
        ))),
    }
}

struct ConfigFragment {
    root: Option<Value>,
    ignore: Vec<Value>,
    extends: Vec<String>,
    require: Vec<String>,
    check_settings: Object,
    context: Option<Mode>,
}

fn as_array(v: Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a,
        other => vec![other],
    }
}

fn truthy_take(o: &mut Object, key: &str) -> Option<Value> {
    if o.get(key).is_some_and(Value::is_truthy) {
        o.remove(key)
    } else {
        None
    }
}

const BUILTIN_RECOMMENDED: &str = "theme-check:recommended";

fn read_yaml_config_description(absolute_path: &str, is_root_config: bool) -> Result<ConfigFragment, ConfigError> {
    let root = uri::posix_dirname(absolute_path);
    let contents = builtin_config(absolute_path).map(Ok).unwrap_or_else(|| {
        fs::read(absolute_path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .map_err(|e| fs_error(&e, "open", absolute_path))
    })?;
    let mut yaml_file = parse_yaml_file(absolute_path, &contents)?;

    let mut config = ConfigFragment {
        root: None,
        ignore: Vec::new(),
        extends: Vec::new(),
        require: Vec::new(),
        check_settings: Object::new(),
        context: None,
    };

    if let Some(r) = truthy_take(&mut yaml_file, "root") {
        config.root = Some(r);
    }
    if let Some(i) = truthy_take(&mut yaml_file, "ignore") {
        config.ignore = as_array(i);
    }
    if let Some(r) = truthy_take(&mut yaml_file, "require") {
        for p in as_array(r) {
            config.require.push(resolve_path_value(&root, &p)?);
        }
    }
    if let Some(e) = truthy_take(&mut yaml_file, "extends") {
        for p in as_array(e) {
            if let Some(ex) = resolve_extends(&root, &p)? {
                config.extends.push(ex);
            }
        }
    } else if is_root_config {
        config.extends = vec![BUILTIN_RECOMMENDED.to_string()];
    }
    if let Some(c) = truthy_take(&mut yaml_file, "context") {
        match c.as_str() {
            Some("theme") => config.context = Some(Mode::Theme),
            Some("app") => config.context = Some(Mode::App),
            _ => {}
        }
    }
    truthy_take(&mut yaml_file, "include_categories");
    truthy_take(&mut yaml_file, "exclude_categories");

    for (check_name, settings) in yaml_file.iter() {
        let Value::Object(settings) = settings else {
            return Err(ConfigError::error(format!(
                "Expected a plain object value for {check_name} but got {}",
                settings.type_of()
            )));
        };
        config.check_settings.insert(check_name, Value::Object(resolve_settings(settings)?));
    }
    Ok(config)
}

fn legacy_identifier(s: &str) -> Option<&'static str> {
    Some(match s {
        "default" | "recommended" => "theme-check:recommended",
        "all" => "theme-check:all",
        "nothing" => "theme-check:nothing",
        "theme_app_extension" | "theme_app_extensions" | "theme-app-extension" | "theme-app-extensions" => {
            "theme-check:theme-app-extension"
        }
        _ => return None,
    })
}

fn resolve_extends(root: &str, path_like: &Value) -> Result<Option<String>, ConfigError> {
    let Value::String(s) = path_like else {
        return Err(match path_like {
            Value::Null => ConfigError::type_error("Cannot read properties of null (reading 'startsWith')"),
            _ => ConfigError::type_error("pathLike.startsWith is not a function"),
        });
    };
    if s.starts_with(':') || legacy_identifier(s).is_some() {
        return Ok(legacy_identifier(s.strip_prefix(':').unwrap_or(s)).map(str::to_string));
    }
    if s.starts_with("theme-check:") {
        return Ok(Some(s.clone()));
    }
    resolve_path(root, s).map(Some)
}

/// `path.isAbsolute(thing)`'s `ERR_INVALID_ARG_TYPE` message.
fn invalid_path_type(v: &Value) -> ConfigError {
    let received = match v {
        Value::Null => "Received null".to_string(),
        Value::Bool(b) => format!("Received type boolean ({b})"),
        Value::Number(n) => format!("Received type number ({})", lq_json::js::number_to_string(*n)),
        Value::Array(_) => "Received an instance of Array".to_string(),
        Value::Object(_) => "Received an instance of Object".to_string(),
        Value::String(_) => unreachable!("strings are valid paths"),
    };
    ConfigError::type_error(format!("The \"path\" argument must be of type string. {received}"))
}

fn resolve_path_value(root: &str, v: &Value) -> Result<String, ConfigError> {
    match v {
        Value::String(s) => resolve_path(root, s),
        other => Err(invalid_path_type(other)),
    }
}

fn resolve_path(root: &str, path_like: &str) -> Result<String, ConfigError> {
    if is_absolute(path_like) {
        return Ok(path_like.to_string());
    }
    if path_like.starts_with('.') {
        return Ok(path_resolve(root, path_like));
    }
    let resolved = node_require_resolve(root, path_like)?;
    fs::canonicalize(&resolved)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| fs_error(&e, "lstat", &resolved))
}

fn to_camel_case(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut last = 0;
    while i < b.len() {
        if b[i] == b'_' && b.get(i + 1).is_some_and(u8::is_ascii_lowercase) {
            out.push_str(&s[last..i]);
            out.push(b[i + 1].to_ascii_uppercase() as char);
            i += 2;
            last = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&s[last..]);
    out
}

fn resolve_settings(settings: &Object) -> Result<Object, ConfigError> {
    let mut resolved = Object::new();
    let enabled = match settings.get("enabled") {
        Some(v) if v.is_truthy() => v.clone(),
        _ => Value::Bool(true),
    };
    resolved.insert("enabled", enabled);
    for (key, value) in settings.iter() {
        resolved.insert(to_camel_case(key), value.clone());
    }
    if let Some(ignore) = settings.get("ignore") {
        if let Some(a) = ignore.as_array() {
            if a.iter().all(|x| matches!(x, Value::String(_))) {
                resolved.insert("ignore", ignore.clone());
            }
        }
    }
    if let Some(severity) = settings.get("severity") {
        resolved.insert("severity", resolve_severity(severity)?);
    }
    Ok(resolved)
}

fn resolve_severity(severity: &Value) -> Result<Value, ConfigError> {
    match severity {
        Value::String(s) => match s.as_str() {
            "suggestion" | "warning" => return Ok(Value::Number(1.0)),
            "style" | "info" => return Ok(Value::Number(2.0)),
            "error" => return Ok(Value::Number(0.0)),
            _ => {}
        },
        Value::Number(n) if *n == 0.0 || *n == 1.0 || *n == 2.0 => return Ok(Value::Number(n.abs())),
        _ => {}
    }
    Err(ConfigError::error(format!(
        "Unsupported severity: {}. Try one of suggestion,style,error,warning,info",
        js_string(severity)
    )))
}

// ---------------------------------------------------------------------------
// merge-fragments.ts, resolve-config.ts
// ---------------------------------------------------------------------------

struct ConfigDescription {
    root: Option<Value>,
    ignore: Vec<Value>,
    require: Vec<String>,
    check_settings: Object,
    context: Mode,
}

fn merge_deep(target: &Object, source: &Object) -> Object {
    let mut output = target.clone();
    for (key, value) in source.iter() {
        match (target.get(key), value) {
            (Some(Value::Object(a)), Value::Object(b)) => {
                output.insert(key, Value::Object(merge_deep(a, b)));
            }
            _ => output.insert(key, value.clone()),
        }
    }
    output
}

/// `base.concat(curr).filter(Boolean).pop()`: `concat` spreads an array value.
fn select_latest_root(base: &[ConfigDescription], curr: &Option<Value>) -> Option<Value> {
    let mut all: Vec<Value> = base.iter().filter_map(|b| b.root.clone()).collect();
    match curr {
        Some(Value::Array(a)) => all.extend(a.iter().cloned()),
        Some(v) => all.push(v.clone()),
        None => {}
    }
    all.retain(Value::is_truthy);
    all.pop()
}

fn merge_fragments(base: Vec<ConfigDescription>, config: ConfigFragment) -> ConfigDescription {
    let root = select_latest_root(&base, &config.root);
    let context = config.context.or_else(|| base.last().map(|b| b.context)).unwrap_or(Mode::Theme);
    let mut ignore = Vec::new();
    let mut require = Vec::new();
    let mut check_settings: Option<Object> = None;
    for b in base {
        ignore.extend(b.ignore);
        require.extend(b.require);
        check_settings = Some(match check_settings {
            Some(cs) => merge_deep(&cs, &b.check_settings),
            None => b.check_settings,
        });
    }
    ignore.extend(config.ignore);
    require.extend(config.require);
    let check_settings = match check_settings {
        Some(cs) => merge_deep(&cs, &config.check_settings),
        None => config.check_settings,
    };
    ConfigDescription { root, ignore, require, check_settings, context }
}

const BUILTIN_DIR: &str = "\0lq-builtin-config/";

fn builtin_config(path: &str) -> Option<String> {
    let name = path.strip_prefix(BUILTIN_DIR)?;
    Some(
        match name {
            "nothing" => include_str!("../configs/nothing.yml"),
            "recommended" => include_str!("../configs/recommended.yml"),
            "theme-app-extension" => include_str!("../configs/theme-app-extension.yml"),
            "all" => include_str!("../configs/all.yml"),
            _ => return None,
        }
        .to_string(),
    )
}

fn modern_identifier(s: &str) -> Option<&str> {
    let name = s.strip_prefix("theme-check:")?;
    matches!(name, "nothing" | "recommended" | "theme-app-extension" | "all").then_some(name)
}

fn resolve_config(
    config_path: &str,
    is_root_config: bool,
    extended_by: &[String],
) -> Result<ConfigDescription, ConfigError> {
    if let Some(name) = modern_identifier(config_path) {
        return resolve_config(&format!("{BUILTIN_DIR}{name}"), false, extended_by);
    }
    let mut chain = extended_by.to_vec();
    chain.push(config_path.to_string());
    if extended_by.iter().any(|p| p == config_path) {
        return Err(ConfigError::error(format!("Cyclic extends in theme-check config: {}", chain.join(" -> "))));
    }
    let current = read_yaml_config_description(config_path, is_root_config)?;
    let mut base = Vec::with_capacity(current.extends.len());
    for e in &current.extends {
        base.push(resolve_config(e, false, &chain)?);
    }
    Ok(merge_fragments(base, current))
}

// ---------------------------------------------------------------------------
// Node module resolution (createRequire(...).resolve), minimal
// ---------------------------------------------------------------------------

fn is_file(p: &str) -> bool {
    fs::metadata(p).is_ok_and(|m| m.is_file())
}

fn is_dir(p: &str) -> bool {
    fs::metadata(p).is_ok_and(|m| m.is_dir())
}

fn try_file_like(p: &str) -> Option<String> {
    if is_file(p) {
        return Some(p.to_string());
    }
    for ext in [".js", ".json", ".node"] {
        let c = format!("{p}{ext}");
        if is_file(&c) {
            return Some(c);
        }
    }
    None
}

fn try_directory(p: &str) -> Option<String> {
    if !is_dir(p) {
        return None;
    }
    let pkg = read_package_json(p);
    if let Some(main) = pkg.as_ref().and_then(|v| v.get("main")).and_then(Value::as_str) {
        let m = path_resolve(p, main);
        if let Some(r) = try_file_like(&m) {
            return Some(r);
        }
        if let Some(r) = try_index(&m) {
            return Some(r);
        }
    }
    try_index(p)
}

fn try_index(dir: &str) -> Option<String> {
    for name in ["index.js", "index.json", "index.node"] {
        let c = format!("{dir}/{name}");
        if is_file(&c) {
            return Some(c);
        }
    }
    None
}

fn read_package_json(dir: &str) -> Option<Value> {
    let text = fs::read_to_string(format!("{dir}/package.json")).ok()?;
    lq_json::parse_json(&text)
}

fn exports_target(target: &Value, star: Option<&str>) -> Option<String> {
    match target {
        Value::String(s) => Some(match star {
            Some(m) => s.replace('*', m),
            None => s.clone(),
        }),
        Value::Object(o) => {
            for (cond, t) in o.iter() {
                if matches!(cond, "node" | "require" | "module-sync" | "node-addons" | "default") {
                    if let Some(r) = exports_target(t, star) {
                        return Some(r);
                    }
                }
            }
            None
        }
        Value::Array(a) => a.iter().find_map(|t| exports_target(t, star)),
        _ => None,
    }
}

fn resolve_exports(pkg_dir: &str, exports: &Value, subpath: &str) -> Result<String, ConfigError> {
    let not_exported = || {
        ConfigError::error(if subpath == "." {
            format!("No \"exports\" main defined in {pkg_dir}/package.json")
        } else {
            format!("Package subpath '{subpath}' is not defined by \"exports\" in {pkg_dir}/package.json")
        })
    };
    let is_sugar = match exports {
        Value::Object(o) => o.keys().next().is_none_or(|k| !k.starts_with('.')),
        _ => true,
    };
    let target = if is_sugar {
        if subpath != "." {
            return Err(not_exported());
        }
        exports_target(exports, None)
    } else {
        let o = exports.as_object().expect("object");
        if let Some(t) = o.get(subpath) {
            exports_target(t, None)
        } else {
            let mut best: Option<(&str, &Value, String)> = None;
            for (k, t) in o.iter() {
                if let Some((pre, post)) = k.split_once('*') {
                    if subpath.starts_with(pre)
                        && subpath.ends_with(post)
                        && subpath.len() >= pre.len() + post.len()
                        && best.as_ref().is_none_or(|b| k.len() > b.0.len())
                    {
                        let m = subpath[pre.len()..subpath.len() - post.len()].to_string();
                        best = Some((k, t, m));
                    }
                }
            }
            best.and_then(|(_, t, m)| exports_target(t, Some(&m)))
        }
    };
    let target = target.ok_or_else(not_exported)?;
    Ok(path_resolve(pkg_dir, &target))
}

fn node_require_resolve(root: &str, request: &str) -> Result<String, ConfigError> {
    let (name, sub) = {
        let mut it = request.splitn(if request.starts_with('@') { 3 } else { 2 }, '/');
        let first = it.next().unwrap_or("");
        if request.starts_with('@') {
            let second = it.next();
            let rest = it.next();
            (second.map_or(first.to_string(), |s| format!("{first}/{s}")), rest.map(str::to_string))
        } else {
            (first.to_string(), it.next().map(str::to_string))
        }
    };
    let mut dir = root.to_string();
    loop {
        if uri::posix_basename(&dir) != "node_modules" {
            let nm = path_join(&dir, "node_modules");
            if is_dir(&nm) {
                let pkg_dir = format!("{nm}/{name}");
                if let Some(exports) = read_package_json(&pkg_dir).and_then(|p| p.get("exports").cloned()) {
                    if !exports.is_null() {
                        let subpath = sub.as_ref().map_or(".".to_string(), |s| format!("./{s}"));
                        let target = resolve_exports(&pkg_dir, &exports, &subpath)?;
                        if is_file(&target) {
                            return Ok(target);
                        }
                        return Err(cannot_find_module(&target, root));
                    }
                }
                let full = format!("{nm}/{request}");
                if let Some(r) = try_file_like(&full).or_else(|| try_directory(&full)) {
                    return Ok(r);
                }
            }
        }
        let parent = uri::posix_dirname(&dir);
        if parent == dir {
            break;
        }
        dir = parent;
    }
    Err(cannot_find_module(request, root))
}

fn cannot_find_module(request: &str, root: &str) -> ConfigError {
    ConfigError::error(format!(
        "Cannot find module '{request}'\nRequire stack:\n- {}",
        path_join(root, "__placeholder")
    ))
}

// ---------------------------------------------------------------------------
// load-config-description.ts
// ---------------------------------------------------------------------------

fn find_node_module_root(root: &str) -> String {
    let mut curr = root.to_string();
    loop {
        let parent = uri::posix_dirname(&curr);
        if parent == curr
            || Path::new(&path_join(&curr, "node_modules")).exists()
            || Path::new(&path_join(&curr, ".git")).exists()
        {
            return curr;
        }
        curr = parent;
    }
}

/// The directories `findThirdPartyChecks(nodeModuleRoot)` would load.
fn find_third_party_checks(node_module_root: &str) -> Vec<String> {
    let mut found = Vec::new();
    let nm = path_join(node_module_root, "node_modules");
    let Ok(entries) = fs::read_dir(&nm) else { return found };
    let is_theme_check = |n: &str| n.starts_with("theme-check-");
    let is_directory = |p: &str| is_dir(p);
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('@') {
            let Ok(inner) = fs::read_dir(format!("{nm}/{name}")) else { continue };
            for i in inner.flatten() {
                let n = i.file_name().to_string_lossy().into_owned();
                let p = format!("{nm}/{name}/{n}");
                let excluded = name == "@shopify"
                    && matches!(
                        n.as_str(),
                        "theme-check-node" | "theme-check-common" | "theme-check-browser" | "theme-check-docs-updater"
                    );
                if is_theme_check(&n) && !excluded && !n.contains("theme-check-vscode") && is_directory(&p) {
                    found.push(p);
                }
            }
        } else if is_theme_check(&name) && !name.contains("theme-check-vscode") && is_directory(&format!("{nm}/{name}")) {
            found.push(format!("{nm}/{name}"));
        }
    }
    found
}

fn is_enabled_by(settings: &Object, check: &CheckInfo) -> bool {
    let check_settings = settings
        .get(check.code)
        .or_else(|| check.aliases.iter().find_map(|a| settings.get(a)));
    check_settings.and_then(|s| s.get("enabled")).is_some_and(Value::is_truthy)
}

/// `Object.fromEntries(entries.map(([code, v]) => [unaliasedCode(code), v]))`: a code
/// not among the enabled checks turns into the key `undefined`.
fn unaliased_settings(settings: &Object, checks: &[&'static CheckInfo]) -> Object {
    let mut out = Object::new();
    for (code, value) in settings.iter() {
        let check = checks.iter().find(|c| c.code == code || c.aliases.contains(&code));
        out.insert(check.map_or("undefined", |c| c.code), value.clone());
    }
    out
}

fn resolve_root(root_uri: &str, path_like: Option<&Value>) -> Result<String, ConfigError> {
    let Some(v) = path_like else { return Ok(root_uri.to_string()) };
    let Value::String(s) = v else { return Err(invalid_path_type(v)) };
    if is_absolute(s) {
        return Err(ConfigError::error("the \"root\" property can only be relative"));
    }
    Ok(uri::resolve(root_uri, s))
}

fn load_config_description(
    description: ConfigDescription,
    root: &str,
    registry: &'static [CheckInfo],
) -> Result<Config, ConfigError> {
    let node_module_root = find_node_module_root(root);
    let mut third_party = description.require.clone();
    third_party.extend(find_third_party_checks(&node_module_root));
    for module in &third_party {
        eprintln!("[lq] note: third-party checks are not supported, ignoring {module}");
    }

    let enabled: Vec<&'static CheckInfo> =
        registry.iter().filter(|c| is_enabled_by(&description.check_settings, c)).collect();
    let root_uri = uri::file_uri(root);
    let settings = unaliased_settings(&description.check_settings, &enabled);
    let root_uri = resolve_root(&root_uri, description.root.as_ref())?;
    let ignore: Vec<String> = description.ignore.iter().filter_map(Value::as_str).map(str::to_string).collect();

    let mut config = Config::new_for_test(
        description.context,
        settings,
        enabled.iter().map(|c| c.code).collect(),
        root_uri,
        Some(ignore),
    );
    config.ignore_values = Some(description.ignore);
    config.require = description.require;

    validate_config(&config, &enabled)?;
    Ok(config)
}

// ---------------------------------------------------------------------------
// validation.ts
// ---------------------------------------------------------------------------

const PROTO_KEYS: &[&str] = &[
    "__proto__",
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
];

fn validate_config(config: &Config, checks: &[&'static CheckInfo]) -> Result<(), ConfigError> {
    for check in checks {
        let Some(settings) = config.settings.get(check.code) else {
            return Err(ConfigError::assertion(format!(
                "Unexpected missing settings for {} in validateConfig call",
                check.code
            )));
        };
        let empty = Object::new();
        let settings_obj = settings.as_object().unwrap_or(&empty);
        for (key, prop) in check.schema {
            validate_value(key, settings_obj.get(key), prop)?;
        }
        if let Some(sev) = settings_obj.get("severity") {
            assert_type("severity", "number", sev.type_of())?;
        }
        let enabled_ty = settings_obj.get("enabled").map_or("undefined", Value::type_of);
        assert_type("enabled", "boolean", enabled_ty)?;
        if let Some(ignore) = settings_obj.get("ignore") {
            assert_array("ignore", ignore)?;
        }
        for key in settings_obj.keys() {
            let in_schema = check.schema.iter().any(|(k, _)| *k == key) || PROTO_KEYS.contains(&key);
            if !in_schema && !matches!(key, "enabled" | "severity" | "ignore") {
                eprintln!("Unexpected setting: {key} for check {} found in configuration", check.code);
            }
        }
    }
    Ok(())
}

fn validate_value(key: &str, value: Option<&Value>, prop: &SchemaProp) -> Result<(), ConfigError> {
    let Some(value) = value else {
        if !prop.optional && prop.default.is_none() {
            return Err(ConfigError::error(format!("Missing required setting: {key}")));
        }
        return Ok(());
    };
    if prop.default.is_some_and(|d| d.identical_to(value)) {
        return Ok(());
    }
    match prop.ty {
        SchemaType::String => assert_type(key, "string", value.type_of()),
        SchemaType::Number => assert_type(key, "number", value.type_of()),
        SchemaType::Boolean => assert_type(key, "boolean", value.type_of()),
        SchemaType::Object => {
            let Value::Object(o) = value else {
                return Err(ConfigError::error(format!(
                    "Invalid type for setting {key}: expected object, got {}",
                    value.type_of()
                )));
            };
            for (nested, nested_prop) in prop.properties {
                validate_value(nested, o.get(nested), nested_prop)?;
            }
            Ok(())
        }
        SchemaType::Array => {
            assert_array(key, value)?;
            if let (Some(item), Value::Array(items)) = (prop.item_type, value) {
                for i in items {
                    validate_value(key, Some(i), item)?;
                }
            }
            Ok(())
        }
    }
}

fn assert_type(key: &str, expected: &str, actual: &str) -> Result<(), ConfigError> {
    if actual == expected {
        return Ok(());
    }
    let message = format!("Invalid type for setting {key}: expected {expected}, got {actual}");
    // Node 22's `assert.strictEqual` appends a diff to custom messages, as
    // `getSimpleDiff` does it when stderr has no colors.
    let diff = if actual.len() + expected.len() <= 12 {
        format!("\n\n'{actual}' !== '{expected}'\n")
    } else {
        format!("\n+ actual - expected\n\n+ '{actual}'\n- '{expected}'\n")
    };
    Err(ConfigError::assertion(format!("{message}{diff}")))
}

fn assert_array(key: &str, thing: &Value) -> Result<(), ConfigError> {
    if matches!(thing, Value::Array(_)) {
        return Ok(());
    }
    Err(ConfigError::assertion(format!(
        "Invalid type for setting {key}: expected array, got {}",
        thing.type_of()
    )))
}

// ---------------------------------------------------------------------------
// load-config.ts, find-config-path.ts, index.ts `loadConfig`
// ---------------------------------------------------------------------------

/// `findConfigPath(root)`.
pub fn find_config_path(root: &str) -> Option<String> {
    let p = path_join(root, ".theme-check.yml");
    fs::metadata(&p).is_ok().then_some(p)
}

/// `loadConfig(configPath, root)` from theme-check-node's `index.ts`.
pub fn load_config(
    config_path: Option<&str>,
    root: &str,
    registry: &'static [CheckInfo],
) -> Result<Config, ConfigError> {
    let found;
    let config_path = match config_path {
        Some(p) => Some(p),
        None => {
            found = find_config_path(root);
            found.as_deref()
        }
    };
    if root.is_empty() {
        return Err(ConfigError::error("loadConfig cannot be called without a root argument"));
    }
    let mut default_checks = BUILTIN_RECOMMENDED;
    if config_path.is_none_or(str::is_empty) {
        let entries = fs::read_dir(root).map_err(|e| fs_error(&e, "scandir", root))?;
        let app = entries.flatten().any(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.ends_with(".extension.toml") || n.ends_with(".app.toml")
        });
        if app {
            default_checks = "theme-check:theme-app-extension";
        }
    }
    let description = resolve_config(config_path.unwrap_or(default_checks), true, &[])?;
    load_config_description(description, root, registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_case() {
        assert_eq!(to_camel_case("max_size_bytes"), "maxSizeBytes");
        assert_eq!(to_camel_case("a_B_c"), "a_BC");
    }

    #[test]
    fn builtin_configs_parse() {
        for n in ["nothing", "recommended", "theme-app-extension", "all"] {
            assert!(resolve_config(&format!("theme-check:{n}"), false, &[]).is_ok());
        }
    }

    #[test]
    fn severity_names() {
        assert_eq!(resolve_severity(&Value::String("style".into())), Ok(Value::Number(2.0)));
        assert!(resolve_severity(&Value::Number(3.0)).is_err());
    }
}
