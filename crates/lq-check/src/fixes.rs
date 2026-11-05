//! `fixes/*`: correctors, applying fixes, autofix.
//!
//! Fixes are data, in byte offsets. A corrector's closure runs when the problem is
//! built, unlike upstream's lazy fixers; only a JSON fix can fail, when applied.

use lq_json::jsonc::{parse, ParseOptions};
use lq_json::{Object, Value};

/// `FixDescription` in byte offsets: `[start, end)` is replaced with `insert`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixDescription {
    pub start: u32,
    pub end: u32,
    pub insert: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum JsonPatch {
    Add { path: String, value: Value },
    Remove { path: String },
}

/// What a fixer collected.
#[derive(Clone, Debug, PartialEq)]
pub enum Fix {
    Edits(Vec<FixDescription>),
    Json(Vec<JsonPatch>),
}

/// A corrector kind. See [`StringCorrector`] and [`JsonCorrector`].
pub trait Corrector: Default {
    fn into_fix(self) -> Fix;
}

/// `StringCorrector`: for Liquid files. Offsets refer to the unmodified file.
#[derive(Clone, Debug, Default)]
pub struct StringCorrector {
    edits: Vec<FixDescription>,
}

impl StringCorrector {
    /// Inserts `text` before `index`.
    pub fn insert(&mut self, index: u32, text: impl Into<String>) {
        self.edits.push(FixDescription { start: index, end: index, insert: text.into() });
    }

    /// Replaces `[start, end)` with `text`.
    pub fn replace(&mut self, start: u32, end: u32, text: impl Into<String>) {
        self.edits.push(FixDescription { start, end, insert: text.into() });
    }

    /// Removes `[start, end)`.
    pub fn remove(&mut self, start: u32, end: u32) {
        self.edits.push(FixDescription { start, end, insert: String::new() });
    }
}

impl Corrector for StringCorrector {
    fn into_fix(self) -> Fix {
        Fix::Edits(self.edits)
    }
}

/// `JSONCorrector`: for JSON files. Paths are lodash paths (`a.b[0].c`).
#[derive(Clone, Debug, Default)]
pub struct JsonCorrector {
    patches: Vec<JsonPatch>,
}

impl JsonCorrector {
    /// Adds `value` at the path, creating containers as needed.
    pub fn add(&mut self, path: impl Into<String>, value: Value) {
        self.patches.push(JsonPatch::Add { path: path.into(), value });
    }

    /// Replaces the value at the path.
    pub fn replace(&mut self, path: impl Into<String>, value: Value) {
        let path = path.into();
        self.patches.push(JsonPatch::Remove { path: path.clone() });
        self.patches.push(JsonPatch::Add { path, value });
    }

    /// Removes the key at the path.
    pub fn remove(&mut self, path: impl Into<String>) {
        self.patches.push(JsonPatch::Remove { path: path.into() });
    }
}

impl Corrector for JsonCorrector {
    fn into_fix(self) -> Fix {
        Fix::Json(self.patches)
    }
}

/// lodash's `stringToPath`, for the forms the correctors see.
fn to_path(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = path.chars().peekable();
    if path.starts_with('.') {
        out.push(String::new());
    }
    while let Some(c) = chars.next() {
        match c {
            '.' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                } else if chars.peek() == Some(&'.') {
                    out.push(String::new());
                }
            }
            '[' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                let mut inner = String::new();
                let mut closed = false;
                for d in chars.by_ref() {
                    if d == ']' {
                        closed = true;
                        break;
                    }
                    inner.push(d);
                }
                if !closed {
                    cur.push('[');
                    cur.push_str(&inner);
                    continue;
                }
                let t = inner.trim_matches(|q| q == '"' || q == '\'');
                out.push(t.to_string());
                if chars.peek() == Some(&'.') {
                    chars.next();
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// lodash's `castPath`: a path that is literally an existing key is that key.
fn cast_path(path: &str, root: &Value) -> Vec<String> {
    let plain = !path.contains(['.', '[', ']']);
    if plain || root.as_object().is_some_and(|o| o.contains_key(path)) {
        vec![path.to_string()]
    } else {
        to_path(path)
    }
}

fn is_index(key: &str) -> bool {
    lq_json::value::array_index(key).is_some()
}

/// `_.set(root, path, value)`.
fn lodash_set(root: &mut Value, path: &str, value: Value) {
    if !matches!(root, Value::Object(_) | Value::Array(_)) {
        return;
    }
    let keys = cast_path(path, root);
    let mut cur = root;
    let n = keys.len();
    for (i, key) in keys.iter().enumerate() {
        if key == "__proto__" || key == "constructor" || key == "prototype" {
            return;
        }
        if i + 1 == n {
            match cur {
                Value::Object(o) => o.insert(key.clone(), value),
                Value::Array(a) => {
                    if let Some(idx) = lq_json::value::array_index(key) {
                        let idx = idx as usize;
                        if idx >= a.len() {
                            a.resize(idx + 1, Value::Null);
                        }
                        a[idx] = value;
                    }
                }
                _ => {}
            }
            return;
        }
        let next_is_index = is_index(&keys[i + 1]);
        let needs_new = match cur {
            Value::Object(o) => !matches!(o.get(key), Some(Value::Object(_) | Value::Array(_))),
            Value::Array(a) => lq_json::value::array_index(key)
                .is_none_or(|j| !matches!(a.get(j as usize), Some(Value::Object(_) | Value::Array(_)))),
            _ => return,
        };
        let fresh = || if next_is_index { Value::Array(Vec::new()) } else { Value::Object(Object::new()) };
        cur = match cur {
            Value::Object(o) => {
                if needs_new {
                    o.insert(key.clone(), fresh());
                }
                o.get_mut(key).expect("just ensured")
            }
            Value::Array(a) => {
                let Some(j) = lq_json::value::array_index(key) else { return };
                let j = j as usize;
                if j >= a.len() {
                    a.resize(j + 1, Value::Null);
                }
                if needs_new {
                    a[j] = fresh();
                }
                &mut a[j]
            }
            _ => return,
        };
    }
}

/// `_.unset(root, path)`.
fn lodash_unset(root: &mut Value, path: &str) {
    let keys = cast_path(path, root);
    let Some((last, parents)) = keys.split_last() else { return };
    let mut cur = root;
    for key in parents {
        cur = match cur {
            Value::Object(o) => match o.get_mut(key) {
                Some(v) => v,
                None => return,
            },
            Value::Array(a) => match lq_json::value::array_index(key).and_then(|j| a.get_mut(j as usize)) {
                Some(v) => v,
                None => return,
            },
            _ => return,
        };
    }
    match cur {
        Value::Object(o) => {
            o.remove(last);
        }
        Value::Array(a) => {
            // `delete arr[i]` leaves a hole, which JSON.stringify prints as null.
            if let Some(v) = lq_json::value::array_index(last).and_then(|j| a.get_mut(j as usize)) {
                *v = Value::Null;
            }
        }
        _ => {}
    }
}

/// Resolves a fix to edits against `source`. `Err` is what `corrector.fix` throws
/// upstream: a JSON fix needs `JSON.parse(source)`, which rejects comments and
/// trailing commas. The message isn't V8's.
pub fn fix_edits(source: &str, fix: &Fix) -> Result<Vec<FixDescription>, String> {
    match fix {
        Fix::Edits(edits) => Ok(edits.clone()),
        Fix::Json(patches) => {
            if patches.is_empty() {
                return Ok(Vec::new());
            }
            let (value, errors) = parse(
                source,
                ParseOptions { disallow_comments: true, allow_trailing_comma: false, allow_empty_content: false },
            );
            if let Some(e) = errors.first() {
                return Err(format!("JSON.parse: {} at byte {}", e.error.message(), e.offset));
            }
            let mut json = value.ok_or_else(|| "Unexpected end of JSON input".to_string())?;
            for patch in patches {
                match patch {
                    JsonPatch::Add { path, value } => lodash_set(&mut json, path, value.clone()),
                    JsonPatch::Remove { path } => lodash_unset(&mut json, path),
                }
            }
            Ok(vec![FixDescription {
                start: 0,
                end: source.len() as u32,
                insert: json.to_json_pretty(2),
            }])
        }
    }
}

/// `applyFixToString(source, fixes)`: fixes all refer to `source`.
pub fn apply_fix_to_string(source: &str, fixes: &[FixDescription]) -> Result<String, String> {
    let mut sorted: Vec<&FixDescription> = fixes.iter().collect();
    sorted.sort_by_key(|f| f.start); // stable, like Array#sort
    for w in sorted.windows(2) {
        if w[0].end > w[1].start {
            return Err("Overlapping ranges are not allowed".into());
        }
    }
    let mut out = String::with_capacity(source.len());
    let mut pos = 0usize;
    for f in sorted {
        let (start, end) = (f.start as usize, f.end as usize);
        if end < start || end > source.len() || !source.is_char_boundary(start) || !source.is_char_boundary(end) {
            return Err("Fix description is going overboard".into());
        }
        out.push_str(&source[pos..start]);
        out.push_str(&f.insert);
        pos = end;
    }
    out.push_str(&source[pos..]);
    Ok(out)
}

/// One source file's outcome of `autofix`: the new contents.
pub fn fix_all(source: &str, fixes: &[&Fix]) -> Result<String, String> {
    let mut edits = Vec::new();
    let mut patches = Vec::new();
    for fix in fixes {
        match fix {
            Fix::Edits(e) => edits.extend(e.iter().cloned()),
            Fix::Json(p) => patches.extend(p.iter().cloned()),
        }
    }
    if !patches.is_empty() {
        edits.extend(fix_edits(source, &Fix::Json(patches))?);
    }
    apply_fix_to_string(source, &edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fd(start: u32, end: u32, insert: &str) -> FixDescription {
        FixDescription { start, end, insert: insert.into() }
    }

    #[test]
    fn apply_matches_upstream_spec() {
        let s = "Hello, world!";
        assert_eq!(apply_fix_to_string(s, &[fd(0, 5, "Hi")]).unwrap(), "Hi, world!");
        assert_eq!(
            apply_fix_to_string(s, &[fd(7, 7, "beautiful "), fd(12, 13, ""), fd(0, 5, "Hi")]).unwrap(),
            "Hi, beautiful world"
        );
        assert_eq!(
            apply_fix_to_string(s, &[fd(5, 7, "w"), fd(6, 8, "b")]).unwrap_err(),
            "Overlapping ranges are not allowed"
        );
        assert_eq!(
            apply_fix_to_string(s, &[fd(6, 6, " wonderful"), fd(6, 7, " and beautiful ")]).unwrap(),
            "Hello, wonderful and beautiful world!"
        );
        assert_eq!(
            apply_fix_to_string(s, &[fd(0, 20, "Hi")]).unwrap_err(),
            "Fix description is going overboard"
        );
    }

    fn json_fix(src: &str, f: impl FnOnce(&mut JsonCorrector)) -> String {
        let mut c = JsonCorrector::default();
        f(&mut c);
        let e = fix_edits(src, &c.into_fix()).unwrap();
        apply_fix_to_string(src, &e).unwrap()
    }

    #[test]
    fn json_corrector_matches_upstream_spec() {
        let s = |v: &str| Value::String(v.into());
        assert_eq!(
            json_fix(r#"{"a":{"b":"b"}}"#, |c| {
                c.add("a.c", s("c"));
                c.add("a.d", s("d"));
            }),
            "{\n  \"a\": {\n    \"b\": \"b\",\n    \"c\": \"c\",\n    \"d\": \"d\"\n  }\n}"
        );
        assert_eq!(
            json_fix(r#"{"a":{"b":"b"}}"#, |c| {
                c.add("a.b.c", s("c"));
            }),
            "{\n  \"a\": {\n    \"b\": {\n      \"c\": \"c\"\n    }\n  }\n}"
        );
        assert_eq!(
            json_fix(r#"{"a":{"b":"b"},"f":{"g":"g"}}"#, |c| {
                c.remove("a.b");
                c.remove("f");
            }),
            "{\n  \"a\": {}\n}"
        );
    }
}
