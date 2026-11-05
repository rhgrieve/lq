//! `jsonSchemaService.js`: the schema handles, `$ref` resolution and file matching.
//!
//! Upstream resolves references by mutating plain JS objects in place, and those
//! objects are shared between schemas: a merge copies keys, not values. Which keys
//! a merge sees depends on what already ran, so this module keeps that model: an
//! arena of objects with identity, the same traversal order, and a job queue that
//! runs the `.then` callbacks in the order the microtask queue would.

use super::eval::Compiled;
use super::glob;
use super::url;
use super::{JsError, SchemaDefinition};
use crate::js;
use crate::jsonc::{self, ParseOptions};
use crate::uri::{decode_uri_component, Uri};
use crate::value::{array_index, Value};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};

/// `services/configuration.js`'s `schemaContributions.schemas`, dumped as JSON.
const META_SCHEMAS: &str = include_str!("meta_schemas.json");
const CONTRIBUTED_SCHEMAS: [&str; 4] = [
    "https://json-schema.org/draft-04/schema",
    "https://json-schema.org/draft-07/schema",
    "https://json-schema.org/draft/2020-12/schema",
    "https://json-schema.org/draft/2019-09/schema",
];

/// Far above what terminating resolutions need, a few hundred jobs.
const MAX_JOBS: u32 = 1 << 16;

/// A JS value inside the schema graph. Objects and arrays have identity.
#[derive(Clone, Debug)]
pub(super) enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(Arc<str>),
    Arr(u32),
    Obj(u32),
}

impl J {
    pub fn truthy(&self) -> bool {
        match self {
            J::Null => false,
            J::Bool(b) => *b,
            J::Num(n) => *n != 0.0 && !n.is_nan(),
            J::Str(s) => !s.is_empty(),
            J::Arr(_) | J::Obj(_) => true,
        }
    }

    pub fn as_str(&self) -> Option<&Arc<str>> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_num(&self) -> Option<f64> {
        match self {
            J::Num(n) => Some(*n),
            _ => None,
        }
    }
}

/// `{}`, for `asSchema(true)` and everything else without schema keywords.
pub(super) const EMPTY_OBJECT: u32 = 0;
/// `{ not: {} }`, for `asSchema(false)`.
pub(super) const FALSE_OBJECT: u32 = 1;

#[derive(Debug, Default)]
pub(super) struct ObjSlot {
    pub entries: Vec<(Arc<str>, J)>,
    pub compiled: OnceLock<Box<Compiled>>,
}

#[derive(Debug)]
pub(super) struct Arena {
    pub objs: Vec<ObjSlot>,
    pub arrs: Vec<Vec<J>>,
}

impl Arena {
    fn new() -> Arena {
        let mut arena = Arena {
            objs: Vec::new(),
            arrs: Vec::new(),
        };
        arena.objs.push(ObjSlot::default());
        arena.objs.push(ObjSlot {
            entries: vec![(Arc::from("not"), J::Obj(EMPTY_OBJECT))],
            compiled: OnceLock::new(),
        });
        arena
    }

    fn intern(&mut self, value: Value) -> J {
        match value {
            Value::Null => J::Null,
            Value::Bool(b) => J::Bool(b),
            Value::Number(n) => J::Num(n),
            Value::String(s) => J::Str(Arc::from(s)),
            Value::Array(items) => {
                let items = items.into_iter().map(|v| self.intern(v)).collect();
                self.arrs.push(items);
                J::Arr(self.arrs.len() as u32 - 1)
            }
            Value::Object(o) => {
                let entries: Vec<(Arc<str>, J)> = o
                    .iter()
                    // `obj['__proto__'] = v` in `Json.parse` makes no own property. It
                    // can set the prototype, which is not modelled.
                    .filter(|(k, _)| *k != "__proto__")
                    .map(|(k, v)| (Arc::from(k), self.intern(v.clone())))
                    .collect();
                self.objs.push(ObjSlot {
                    entries,
                    compiled: OnceLock::new(),
                });
                J::Obj(self.objs.len() as u32 - 1)
            }
        }
    }

    pub fn get(&self, obj: u32, key: &str) -> Option<&J> {
        self.objs[obj as usize]
            .entries
            .iter()
            .find(|(k, _)| &**k == key)
            .map(|(_, v)| v)
    }

    /// `schema[key]` for any schema value: only objects have keywords.
    pub fn get_j(&self, value: &J, key: &str) -> Option<&J> {
        match value {
            J::Obj(o) => self.get(*o, key),
            _ => None,
        }
    }

    pub fn arr(&self, value: Option<&J>) -> Option<&[J]> {
        match value {
            Some(J::Arr(a)) => Some(&self.arrs[*a as usize]),
            _ => None,
        }
    }

    fn set(&mut self, obj: u32, key: Arc<str>, value: J) {
        let entries = &mut self.objs[obj as usize].entries;
        if let Some(slot) = entries.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = value;
            return;
        }
        if let Some(n) = array_index(&key) {
            let at = entries
                .iter()
                .position(|(k, _)| array_index(k).is_none_or(|m| m > n))
                .unwrap_or(entries.len());
            entries.insert(at, (key, value));
            return;
        }
        entries.push((key, value));
    }

    fn delete(&mut self, obj: u32, key: &str) {
        let entries = &mut self.objs[obj as usize].entries;
        if let Some(i) = entries.iter().position(|(k, _)| &**k == key) {
            entries.remove(i);
        }
    }

    /// `for (key in value)` / `Object.entries(value)`.
    pub fn own_entries(&self, value: &J) -> Vec<(Arc<str>, J)> {
        match value {
            J::Obj(o) => self.objs[*o as usize].entries.clone(),
            J::Arr(a) => self.arrs[*a as usize]
                .iter()
                .enumerate()
                .map(|(i, v)| (Arc::from(i.to_string()), v.clone()))
                .collect(),
            J::Str(s) => {
                let mut units = Vec::new();
                for c in s.chars() {
                    let text: Arc<str> = Arc::from(c.to_string());
                    units.push(text.clone());
                    if c.len_utf16() == 2 {
                        // The second half of a pair cannot exist alone in a `str`.
                        units.push(text);
                    }
                }
                units
                    .into_iter()
                    .enumerate()
                    .map(|(i, u)| (Arc::from(i.to_string()), J::Str(u)))
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// `JSON.stringify(value)`.
    pub fn stringify(&self, value: &J, out: &mut String, depth: u32) {
        match value {
            J::Null => out.push_str("null"),
            J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            J::Num(n) if n.is_finite() => out.push_str(&js::number_to_string(*n)),
            J::Num(_) => out.push_str("null"),
            J::Str(s) => js::quote_into(s, out),
            // A cycle here throws upstream; the graph has no way to build one short of
            // `$ref`s into `enum` values. Stop instead of overflowing.
            _ if depth > 200 => out.push_str("null"),
            J::Arr(a) => {
                out.push('[');
                for (i, v) in self.arrs[*a as usize].iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    self.stringify(v, out, depth + 1);
                }
                out.push(']');
            }
            J::Obj(o) => {
                out.push('{');
                for (i, (k, v)) in self.objs[*o as usize].entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    js::quote_into(k, out);
                    out.push(':');
                    self.stringify(v, out, depth + 1);
                }
                out.push('}');
            }
        }
    }

    /// `String(value)`.
    pub fn to_js_string(&self, value: &J, depth: u32) -> String {
        match value {
            J::Null => "null".into(),
            J::Bool(b) => b.to_string(),
            J::Num(n) => js::number_to_string(*n),
            J::Str(s) => s.to_string(),
            J::Obj(_) => "[object Object]".into(),
            J::Arr(_) if depth > 200 => String::new(),
            J::Arr(a) => self.join(&self.arrs[*a as usize], ",", depth + 1),
        }
    }

    /// `array.join(separator)`: `null` elements are empty.
    pub fn join(&self, items: &[J], separator: &str, depth: u32) -> String {
        let mut out = String::new();
        for (i, v) in items.iter().enumerate() {
            if i > 0 {
                out.push_str(separator);
            }
            if !matches!(v, J::Null) {
                out.push_str(&self.to_js_string(v, depth));
            }
        }
        out
    }

    /// `l10n.t(template, arg)` for a template with one `{0}`.
    pub fn t1(&self, template: &str, arg: &J) -> Result<String, JsError> {
        let value = match arg {
            J::Null => {
                return Err(JsError::type_error(
                    "Cannot convert undefined or null to object",
                ))
            }
            // An object argument is the record of named values.
            J::Obj(o) => {
                if self.objs[*o as usize].entries.is_empty() {
                    return Ok(template.to_string());
                }
                self.get(*o, "0").cloned()
            }
            J::Arr(a) => {
                if self.arrs[*a as usize].is_empty() {
                    return Ok(template.to_string());
                }
                self.arrs[*a as usize].first().cloned()
            }
            other => Some(other.clone()),
        };
        Ok(match value {
            None | Some(J::Null) => template.to_string(),
            Some(v) => template.replacen("{0}", &self.to_js_string(&v, 0), 1),
        })
    }
}

/// `normalizeId`.
pub(super) fn normalize_id(id: &str) -> String {
    let https;
    let id = match id.strip_prefix("http://json-schema.org/") {
        Some(rest) => {
            https = format!("https://json-schema.org/{rest}");
            https.as_str()
        }
        None => id,
    };
    match Uri::parse(id) {
        Ok(uri) => uri.to_string_with(true),
        Err(_) => id.to_string(),
    }
}

/// `getSchemaDraftFromId`: the numeric `SchemaDraft`.
pub(super) fn schema_draft_from_id(id: &J) -> Result<Option<u8>, JsError> {
    let J::Str(id) = id else {
        return Err(JsError::type_error("id.startsWith is not a function"));
    };
    Ok(match normalize_id(id).as_str() {
        "https://json-schema.org/draft-03/schema" => Some(3),
        "https://json-schema.org/draft-04/schema" => Some(4),
        "https://json-schema.org/draft-06/schema" => Some(6),
        "https://json-schema.org/draft-07/schema" => Some(7),
        "https://json-schema.org/draft/2019-09/schema" => Some(19),
        "https://json-schema.org/draft/2020-12/schema" => Some(20),
        _ => None,
    })
}

fn normalize_resource_for_matching(resource: &str) -> String {
    match Uri::parse(resource) {
        Ok(mut uri) => {
            uri.query.clear();
            uri.fragment.clear();
            uri.to_string_with(true)
        }
        Err(_) => resource.to_string(),
    }
}

fn to_display_string(url: &str) -> String {
    match Uri::parse(url) {
        Ok(uri) if uri.scheme == "file" => uri.fs_path(),
        _ => url.to_string(),
    }
}

/// `FilePatternAssociation` without a `folderUri`, which theme-check never passes.
#[derive(Debug)]
pub(super) struct Association {
    globs: Vec<(glob::Glob, bool)>,
    uri: String,
}

impl Association {
    fn new(patterns: &[String], uri: String) -> Association {
        let mut globs = Vec::new();
        let mut failed = false;
        for pattern in patterns {
            let include = !pattern.starts_with('!');
            let mut p: &str = if include { pattern } else { &pattern[1..] };
            if !p.is_empty() {
                if let Some(rest) = p.strip_prefix('/') {
                    p = rest;
                }
                match glob::Glob::new(&format!("**/{p}")) {
                    Ok(g) => globs.push((g, include)),
                    Err(()) => failed = true,
                }
            }
        }
        if failed {
            globs.clear();
        }
        Association {
            globs,
            uri: if failed { String::new() } else { uri },
        }
    }

    fn matches(&self, file_name: &str) -> bool {
        let mut matched = false;
        for (glob, include) in &self.globs {
            if glob.is_match(file_name) {
                matched = *include;
            }
        }
        matched
    }
}

#[derive(Debug, Default)]
pub(super) struct Resolved {
    pub root: Option<J>,
    pub first_error: Option<String>,
    pub warnings: Vec<String>,
    /// The verdict of `schemaAllowsComments`/`schemaAllowsTrailingCommas`.
    pub allow_check: OnceLock<Result<(), JsError>>,
}

#[derive(Debug)]
enum Waiter {
    Resolve(u32),
    External {
        node: u32,
        segment: Option<String>,
        parent: u32,
        referenced: u32,
    },
}

#[derive(Debug)]
enum Load {
    NotLoaded,
    /// One of upstream's `schemaContributions`: content known, not yet interned.
    Contribution(&'static str),
    Loading(Vec<Waiter>),
    Loaded {
        schema: Option<J>,
        first_error: Option<String>,
    },
}

#[derive(Debug)]
pub(super) struct Handle {
    uri: Arc<str>,
    load: Load,
    anchors: Option<HashMap<String, J>>,
    pub resolved: Option<Result<Resolved, JsError>>,
}

#[derive(Debug)]
enum Job {
    Load(u32),
    Run(Waiter),
}

/// The state of one `resolveSchemaContent` call.
#[derive(Default)]
struct Run {
    errors: Vec<String>,
    unsupported: Vec<&'static str>,
    rejection: Option<JsError>,
}

impl Run {
    fn unsupported(&mut self, feature: &'static str) {
        if !self.unsupported.contains(&feature) {
            self.unsupported.push(feature);
        }
    }
}

#[derive(Debug)]
pub(super) struct Service {
    pub arena: Arena,
    pub handles: Vec<Handle>,
    pub by_id: HashMap<String, u32>,
    pub associations: Vec<Association>,
    /// `cachedSchemaForResource`, only ever needed for a combined schema.
    cached_combined: Option<(String, u32)>,
    queue: VecDeque<Job>,
    seen: Vec<u32>,
    stamp: u32,
    mutated: bool,
}

/// What `getSchemaForResource` picked before any handle exists.
pub(super) enum Target {
    None,
    Id(String),
    Combined(Vec<String>),
}

impl Service {
    /// `service.configure({ schemas })`.
    pub fn new(definitions: &[SchemaDefinition]) -> Service {
        let mut service = Service {
            arena: Arena::new(),
            handles: Vec::new(),
            by_id: HashMap::new(),
            associations: Vec::new(),
            cached_combined: None,
            queue: VecDeque::new(),
            seen: Vec::new(),
            stamp: 0,
            mutated: false,
        };
        // `setSchemaContributions`, which every language service starts with.
        for id in CONTRIBUTED_SCHEMAS {
            let h = service.add_handle(id, Load::Contribution(id));
            service.by_id.insert(id.to_string(), h);
        }
        for definition in definitions {
            let id = normalize_id(&definition.uri);
            if let Some(file_match) = definition.file_match.as_ref().filter(|m| !m.is_empty()) {
                service
                    .associations
                    .push(Association::new(file_match, id.clone()));
            }
            service.get_or_add(&id);
        }
        service
    }

    fn get_or_add(&mut self, id: &str) -> u32 {
        if let Some(&h) = self.by_id.get(id) {
            return h;
        }
        let h = self.add_handle(id, Load::NotLoaded);
        self.by_id.insert(id.to_string(), h);
        h
    }

    fn add_handle(&mut self, id: &str, load: Load) -> u32 {
        self.handles.push(Handle {
            uri: Arc::from(id),
            load,
            anchors: None,
            resolved: None,
        });
        self.handles.len() as u32 - 1
    }

    /// `getAssociatedSchemas`.
    pub fn associated(&self, resource: &str) -> Vec<&str> {
        if self.associations.is_empty() {
            return Vec::new();
        }
        let normalized = normalize_resource_for_matching(resource);
        let mut out: Vec<&str> = Vec::new();
        for entry in &self.associations {
            if entry.matches(&normalized)
                && !entry.uri.is_empty()
                && !out.contains(&entry.uri.as_str())
            {
                out.push(&entry.uri);
            }
        }
        out
    }

    /// The handle for a target, if it needs no more work than a lookup.
    pub fn ready_handle(&self, target: &Target) -> Option<Option<u32>> {
        match target {
            Target::None if self.cached_combined.is_none() => Some(None),
            Target::Id(id) if self.cached_combined.is_none() => {
                let &h = self.by_id.get(id)?;
                self.handles[h as usize]
                    .resolved
                    .is_some()
                    .then_some(Some(h))
            }
            _ => None,
        }
    }

    /// `getSchemaForResource` past the `$schema` check, then `getResolvedSchema`.
    pub fn resolve_target(
        &mut self,
        target: &Target,
        resource: &str,
        from_property: bool,
        definitions: &HashMap<&str, &str>,
    ) -> Option<u32> {
        if !from_property && !matches!(target, Target::Combined(_)) {
            self.cached_combined = None;
        }
        let handle = match target {
            Target::None => return None,
            Target::Id(id) => self.get_or_add(id),
            Target::Combined(ids) => match &self.cached_combined {
                Some((r, h)) if r == resource => *h,
                _ => {
                    let h = self.add_combined(resource, ids);
                    self.cached_combined = Some((resource.to_string(), h));
                    h
                }
            },
        };
        self.ensure_resolved(handle, definitions);
        Some(handle)
    }

    /// `createCombinedSchema` with more than one schema.
    fn add_combined(&mut self, resource: &str, ids: &[String]) -> u32 {
        let refs: Vec<J> = ids
            .iter()
            .map(|id| {
                self.arena.objs.push(ObjSlot {
                    entries: vec![(Arc::from("$ref"), J::Str(Arc::from(id.as_str())))],
                    compiled: OnceLock::new(),
                });
                J::Obj(self.arena.objs.len() as u32 - 1)
            })
            .collect();
        self.arena.arrs.push(refs);
        let all_of = J::Arr(self.arena.arrs.len() as u32 - 1);
        self.arena.objs.push(ObjSlot {
            entries: vec![(Arc::from("allOf"), all_of)],
            compiled: OnceLock::new(),
        });
        let schema = J::Obj(self.arena.objs.len() as u32 - 1);
        let id = format!(
            "schemaservice://combinedSchema/{}",
            encode_uri_component(resource)
        );
        let h = self.add_handle(
            &id,
            Load::Loaded {
                schema: Some(schema),
                first_error: None,
            },
        );
        self.by_id.insert(id, h);
        h
    }

    fn ensure_resolved(&mut self, handle: u32, definitions: &HashMap<&str, &str>) {
        if self.handles[handle as usize].resolved.is_some() {
            return;
        }
        self.then_unresolved(handle, Waiter::Resolve(handle));
        let mut run = Run::default();
        let mut resolved_root: Option<Option<J>> = None;
        let mut jobs = 0u32;
        while let Some(job) = self.queue.pop_front() {
            jobs += 1;
            if jobs > MAX_JOBS {
                // External `$ref`s that cycle through unresolved sections never settle
                // upstream: the promise stays pending while the heap fills up.
                self.queue.clear();
                run.rejection.get_or_insert(JsError {
                    name: "Error".into(),
                    message: "$ref resolution does not terminate".into(),
                });
                break;
            }
            match job {
                Job::Load(h) => self.load_schema(h, definitions),
                Job::Run(Waiter::Resolve(h)) => {
                    if let Err(e) = self.resolve_schema_content(h, &mut run, &mut resolved_root) {
                        run.rejection.get_or_insert(e);
                    }
                }
                Job::Run(Waiter::External {
                    node,
                    segment,
                    parent,
                    referenced,
                }) => {
                    let _ = parent;
                    if let Err(e) = self.run_external(node, segment, referenced, &mut run) {
                        run.rejection.get_or_insert(e);
                    }
                }
            }
        }
        if std::mem::take(&mut self.mutated) {
            // Compiled schemas read keys of shared objects, and any object may have
            // changed. Resolutions are few, so recompile lazily instead of tracking.
            for slot in &mut self.arena.objs {
                slot.compiled = OnceLock::new();
            }
            for h in &mut self.handles {
                if let Some(Ok(r)) = &mut h.resolved {
                    r.allow_check = OnceLock::new();
                }
            }
        }
        let result = match run.rejection {
            Some(e) => Err(e),
            None => {
                let mut warnings = Vec::new();
                if !run.unsupported.is_empty() {
                    warnings.push(format!(
                        "The schema uses meta-schema features ({}) that are not yet supported by the validator.",
                        run.unsupported.join(", ")
                    ));
                }
                Ok(Resolved {
                    root: resolved_root.flatten(),
                    first_error: run.errors.into_iter().next(),
                    warnings,
                    allow_check: OnceLock::new(),
                })
            }
        };
        self.handles[handle as usize].resolved = Some(result);
    }

    /// `handle.getUnresolvedSchema().then(...)`.
    fn then_unresolved(&mut self, handle: u32, waiter: Waiter) {
        match &mut self.handles[handle as usize].load {
            load @ Load::NotLoaded => {
                *load = Load::Loading(vec![waiter]);
                self.queue.push_back(Job::Load(handle));
            }
            Load::Contribution(id) => {
                let id = *id;
                let (value, _) = jsonc::parse(META_SCHEMAS, ParseOptions::default());
                let schema = value
                    .and_then(|v| v.get(id).cloned())
                    .map(|v| self.arena.intern(v));
                self.handles[handle as usize].load = Load::Loaded {
                    schema,
                    first_error: None,
                };
                self.queue.push_back(Job::Run(waiter));
            }
            Load::Loading(waiters) => waiters.push(waiter),
            Load::Loaded { .. } => self.queue.push_back(Job::Run(waiter)),
        }
    }

    /// The fulfilment callback of `loadSchema`, `requestService` being `getSchemaForURI`.
    fn load_schema(&mut self, handle: u32, definitions: &HashMap<&str, &str>) {
        let url = self.handles[handle as usize].uri.clone();
        let missing;
        // `const schema = this.schemas[uri]?.schema; if (!schema) return 'No schema...'`
        let mut content: &str = match definitions.get(&*url) {
            Some(text) if !text.is_empty() => text,
            _ => {
                missing = format!("No schema for '{url}' found");
                &missing
            }
        };
        let mut first_error = None;
        if content.starts_with('\u{FEFF}') {
            first_error = Some(format!(
                "Problem reading content from '{}': UTF-8 with BOM detected, only UTF 8 is allowed.",
                to_display_string(&url)
            ));
            content = content.trim_start_matches(js::is_js_whitespace);
        }
        let (value, errors) = jsonc::parse(content, ParseOptions::default());
        if let Some(error) = errors.first() {
            first_error.get_or_insert_with(|| {
                format!(
                    "Unable to parse content from '{}': Parse error at offset {}.",
                    to_display_string(&url),
                    js::utf16_len(&content[..(error.offset as usize).min(content.len())])
                )
            });
        }
        let schema = value.map(|v| self.arena.intern(v));
        let previous = std::mem::replace(
            &mut self.handles[handle as usize].load,
            Load::Loaded {
                schema,
                first_error,
            },
        );
        if let Load::Loading(waiters) = previous {
            self.queue.extend(waiters.into_iter().map(Job::Run));
        }
    }

    fn resolve_schema_content(
        &mut self,
        handle: u32,
        run: &mut Run,
        resolved_root: &mut Option<Option<J>>,
    ) -> Result<(), JsError> {
        let Load::Loaded {
            schema,
            first_error,
        } = &self.handles[handle as usize].load
        else {
            return Ok(());
        };
        let schema = schema.clone();
        if let Some(e) = first_error {
            run.errors.push(e.clone());
        }
        let root = match &schema {
            None => {
                return Err(JsError::type_error(
                    "Cannot read properties of undefined (reading '$schema')",
                ))
            }
            Some(J::Null) => {
                return Err(JsError::type_error(
                    "Cannot read properties of null (reading '$schema')",
                ))
            }
            Some(root) => root.clone(),
        };
        if let Some(id) = self.arena.get_j(&root, "$schema").filter(|id| id.truthy()) {
            if schema_draft_from_id(id)? == Some(3) {
                run.errors.clear();
                run.errors
                    .push("Draft-03 schemas are not supported.".into());
                *resolved_root = Some(Some(J::Obj(EMPTY_OBJECT)));
                return Ok(());
            }
        }
        *resolved_root = Some(schema.clone());
        self.resolve_refs(&root, &schema, handle, run)
    }

    fn run_external(
        &mut self,
        node: u32,
        segment: Option<String>,
        referenced: u32,
        run: &mut Run,
    ) -> Result<(), JsError> {
        let Load::Loaded {
            schema,
            first_error,
        } = &self.handles[referenced as usize].load
        else {
            return Ok(());
        };
        let schema = schema.clone();
        if let Some(error) = first_error {
            run.errors.push(match segment.as_deref() {
                Some(s) if !s.is_empty() => format!("Problems loading reference '{s}': {error}"),
                _ => error.clone(),
            });
        }
        self.merge_ref(node, &schema, referenced, segment.as_deref(), run)?;
        self.resolve_refs(&J::Obj(node), &schema, referenced, run)
    }

    fn resolve_refs(
        &mut self,
        node: &J,
        parent_schema: &Option<J>,
        parent: u32,
        run: &mut Run,
    ) -> Result<(), JsError> {
        let J::Obj(root) = *node else { return Ok(()) };
        self.seen.resize(self.arena.objs.len(), 0);
        self.stamp += 1;
        let stamp = self.stamp;
        let mut to_walk = vec![root];
        while let Some(next) = to_walk.pop() {
            if self.seen[next as usize] == stamp {
                continue;
            }
            self.seen[next as usize] = stamp;
            self.handle_refs(next, parent_schema, parent, run)?;
            collect_children(&self.arena, next, &mut to_walk);
        }
        Ok(())
    }

    fn handle_refs(
        &mut self,
        next: u32,
        parent_schema: &Option<J>,
        parent: u32,
        run: &mut Run,
    ) -> Result<(), JsError> {
        let mut seen_refs: Vec<Arc<str>> = Vec::new();
        while let Some(value) = self.arena.get(next, "$ref").filter(|v| v.truthy()) {
            let J::Str(reference) = value.clone() else {
                return Err(JsError::type_error("ref.split is not a function"));
            };
            let (external, segment) = match reference.split_once('#') {
                Some((a, rest)) => (a, Some(rest.split('#').next().unwrap_or(""))),
                None => (&*reference, None),
            };
            self.arena.delete(next, "$ref");
            self.mutated = true;
            if !external.is_empty() {
                self.resolve_external(next, external, segment, parent)?;
                return Ok(());
            }
            if !seen_refs.contains(&reference) {
                self.merge_ref(next, parent_schema, parent, segment, run)?;
                seen_refs.push(reference.clone());
            }
        }
        if self.arena.get(next, "$recursiveRef").is_some_and(J::truthy) {
            run.unsupported("$recursiveRef");
        }
        if self.arena.get(next, "$dynamicRef").is_some_and(J::truthy) {
            run.unsupported("$dynamicRef");
        }
        Ok(())
    }

    fn resolve_external(
        &mut self,
        node: u32,
        uri: &str,
        segment: Option<&str>,
        parent: u32,
    ) -> Result<(), JsError> {
        let absolute = has_absolute_scheme(uri);
        let resolved;
        let uri = if absolute {
            uri
        } else {
            resolved = url::resolve(uri, &self.handles[parent as usize].uri)
                .map_err(|()| JsError::type_error("Invalid URL"))?;
            resolved.as_str()
        };
        let id = normalize_id(uri);
        let referenced = self.get_or_add(&id);
        self.then_unresolved(
            referenced,
            Waiter::External {
                node,
                segment: segment.map(str::to_string),
                parent,
                referenced,
            },
        );
        Ok(())
    }

    fn merge_ref(
        &mut self,
        target: u32,
        source_root: &Option<J>,
        source: u32,
        segment: Option<&str>,
        run: &mut Run,
    ) -> Result<(), JsError> {
        let section = match segment {
            None | Some("") => source_root
                .clone()
                .map(Section::Value)
                .unwrap_or(Section::Undefined),
            Some(s) if s.starts_with('/') => self.find_section(source_root, s)?,
            Some(s) => {
                self.find_schema_by_id(source_root, source, run);
                let anchors = self.handles[source as usize].anchors.as_ref();
                match anchors.and_then(|a| a.get(s)) {
                    Some(v) => Section::Value(v.clone()),
                    None => Section::Undefined,
                }
            }
        };
        match section {
            Section::Value(section) if section.truthy() => {
                self.merge(target, &section)?;
            }
            Section::Opaque => {}
            _ => {
                run.errors.push(format!(
                    "$ref '{}' in '{}' can not be resolved.",
                    segment.unwrap_or(""),
                    self.handles[source as usize].uri
                ));
            }
        }
        Ok(())
    }

    fn merge(&mut self, target: u32, section: &J) -> Result<(), JsError> {
        let entries = self.arena.own_entries(section);
        if matches!(section, J::Obj(_))
            && !entries.is_empty()
            && entries.iter().any(|(k, _)| &**k == "hasOwnProperty")
        {
            return Err(JsError::type_error(
                "section.hasOwnProperty is not a function",
            ));
        }
        for (key, value) in entries {
            if &*key != "id" && &*key != "$id" {
                self.arena.set(target, key, value);
                self.mutated = true;
            }
        }
        Ok(())
    }

    /// `findSectionByJSONPointer`.
    fn find_section(&self, root: &Option<J>, path: &str) -> Result<Section, JsError> {
        let path = decode_uri_component(path).ok_or_else(|| JsError {
            name: "URIError".into(),
            message: "URI malformed".into(),
        })?;
        let path = path.strip_prefix('/').unwrap_or(&path);
        let mut current = root.clone().map_or(Section::Undefined, Section::Value);
        for part in path.split('/') {
            let part = part.replace("~1", "/").replace("~0", "~");
            current = match &current {
                Section::Undefined => {
                    return Err(JsError::type_error(&format!(
                        "Cannot read properties of undefined (reading '{part}')"
                    )))
                }
                Section::Value(J::Null) => {
                    return Err(JsError::type_error(&format!(
                        "Cannot read properties of null (reading '{part}')"
                    )))
                }
                Section::Value(value) => self.member(value, &part),
                Section::Opaque => Section::Undefined,
            };
            match &current {
                Section::Undefined => break,
                Section::Value(v) if !v.truthy() => break,
                _ => {}
            }
        }
        Ok(current)
    }

    /// `value[part]`, with inherited `Object.prototype` members as opaque values.
    fn member(&self, value: &J, part: &str) -> Section {
        let own = match value {
            J::Obj(o) => self.arena.get(*o, part).cloned(),
            J::Arr(a) => {
                let items = &self.arena.arrs[*a as usize];
                if part == "length" {
                    Some(J::Num(items.len() as f64))
                } else {
                    array_index(part).and_then(|i| items.get(i as usize).cloned())
                }
            }
            J::Str(s) => {
                if part == "length" {
                    Some(J::Num(js::utf16_len(s) as f64))
                } else {
                    array_index(part).and_then(|i| {
                        self.arena
                            .own_entries(value)
                            .get(i as usize)
                            .map(|(_, v)| v.clone())
                    })
                }
            }
            _ => None,
        };
        match own {
            Some(v) => Section::Value(v),
            None if is_object_prototype_member(part) => Section::Opaque,
            None => Section::Undefined,
        }
    }

    /// The first half of `findSchemaById`: `handle.anchors ??= collectAnchors(schema)`.
    fn find_schema_by_id(&mut self, root: &Option<J>, source: u32, run: &mut Run) {
        if self.handles[source as usize].anchors.is_some() {
            return;
        }
        let mut anchors = HashMap::new();
        if let Some(J::Obj(root)) = root {
            let mut seen = vec![false; self.arena.objs.len()];
            let mut to_walk = vec![*root];
            while let Some(next) = to_walk.pop() {
                if std::mem::replace(&mut seen[next as usize], true) {
                    continue;
                }
                let arena = &self.arena;
                let id = arena
                    .get(next, "$id")
                    .filter(|v| v.truthy())
                    .or_else(|| arena.get(next, "id").filter(|v| v.truthy()));
                let hash_id = id.and_then(J::as_str).filter(|s| s.starts_with('#'));
                let anchor: Option<J> = match hash_id {
                    Some(s) => Some(J::Str(Arc::from(&s[1..]))),
                    None => arena.get(next, "$anchor").cloned(),
                };
                if let Some(anchor) = anchor.filter(J::truthy) {
                    // A non-string anchor can never be the target of a `#fragment`.
                    let key = arena.to_js_string(&anchor, 0);
                    match anchors.entry(key) {
                        Entry::Occupied(e) => {
                            run.errors
                                .push(format!("Duplicate anchor declaration: '{}'", e.key()));
                        }
                        Entry::Vacant(e) => {
                            e.insert(J::Obj(next));
                        }
                    }
                }
                if arena.get(next, "$recursiveAnchor").is_some_and(J::truthy) {
                    run.unsupported("$recursiveAnchor");
                }
                if arena.get(next, "$dynamicAnchor").is_some_and(J::truthy) {
                    run.unsupported("$dynamicAnchor");
                }
                collect_children(arena, next, &mut to_walk);
            }
        }
        self.handles[source as usize].anchors = Some(anchors);
    }
}

enum Section {
    Undefined,
    Value(J),
    /// A function inherited from a prototype: truthy, with no enumerable keys.
    Opaque,
}

fn is_object_prototype_member(key: &str) -> bool {
    matches!(
        key,
        "constructor"
            | "__defineGetter__"
            | "__defineSetter__"
            | "hasOwnProperty"
            | "__lookupGetter__"
            | "__lookupSetter__"
            | "isPrototypeOf"
            | "propertyIsEnumerable"
            | "toString"
            | "valueOf"
            | "__proto__"
            | "toLocaleString"
    )
}

/// `/^[A-Za-z][A-Za-z0-9+\-.+]*:\/.*/`.
fn has_absolute_scheme(uri: &str) -> bool {
    let b = uri.as_bytes();
    if !b.first().is_some_and(u8::is_ascii_alphabetic) {
        return false;
    }
    match b
        .iter()
        .position(|c| !(c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.')))
    {
        Some(i) => b[i] == b':' && b.get(i + 1) == Some(&b'/'),
        None => false,
    }
}

fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The body of `traverseNodes`'s loop. Pushes in upstream's order; the walk pops.
fn collect_children(arena: &Arena, next: u32, to_walk: &mut Vec<u32>) {
    let entries = &arena.objs[next as usize].entries;
    if entries.is_empty() {
        return;
    }
    let lookup = |key: &str| entries.iter().find(|(k, _)| &**k == key).map(|(_, v)| v);
    for key in [
        "additionalItems",
        "additionalProperties",
        "not",
        "contains",
        "propertyNames",
        "if",
        "then",
        "else",
        "unevaluatedItems",
        "unevaluatedProperties",
    ] {
        if let Some(J::Obj(o)) = lookup(key) {
            to_walk.push(*o);
        }
    }
    for key in [
        "definitions",
        "$defs",
        "properties",
        "patternProperties",
        "dependencies",
        "dependentSchemas",
    ] {
        if let Some(J::Obj(map)) = lookup(key) {
            for (_, entry) in &arena.objs[*map as usize].entries {
                if let J::Obj(o) = entry {
                    to_walk.push(*o);
                }
            }
        }
    }
    for key in ["anyOf", "allOf", "oneOf", "prefixItems"] {
        if let Some(J::Arr(a)) = lookup(key) {
            for entry in &arena.arrs[*a as usize] {
                if let J::Obj(o) = entry {
                    to_walk.push(*o);
                }
            }
        }
    }
    match lookup("items") {
        Some(J::Arr(a)) => {
            for entry in &arena.arrs[*a as usize] {
                if let J::Obj(o) = entry {
                    to_walk.push(*o);
                }
            }
        }
        Some(J::Obj(o)) => to_walk.push(*o),
        _ => {}
    }
}
