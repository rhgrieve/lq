//! A JavaScript-flavoured JSON value.

use crate::js;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Object),
}

/// A plain JS object: own keys iterate as JS does, array-index keys first in
/// ascending order, then the rest in insertion order. Setting an existing key
/// keeps its position.
#[derive(Clone, Debug, Default)]
pub struct Object {
    entries: Vec<(String, Value)>,
    /// Lazily maintained for big objects (locale files); `get` is linear below this.
    index: Option<HashMap<String, u32>>,
}

const INDEX_THRESHOLD: usize = 32;

impl PartialEq for Object {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

/// `"12"` is an array index as far as JS key ordering goes: canonical numeric
/// strings below 2^32 - 1.
pub fn array_index(key: &str) -> Option<u32> {
    let b = key.as_bytes();
    if b.is_empty() || b.len() > 10 || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if b[0] == b'0' && b.len() > 1 {
        return None;
    }
    let n: u64 = key.parse().ok()?;
    (n < u64::from(u32::MAX)).then_some(n as u32)
}

impl Object {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn position(&self, key: &str) -> Option<usize> {
        match &self.index {
            Some(index) => index.get(key).map(|&i| i as usize),
            None => self.entries.iter().position(|(k, _)| k == key),
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.position(key).map(|i| &self.entries[i].1)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.position(key).map(|i| &mut self.entries[i].1)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.position(key).is_some()
    }

    pub fn insert(&mut self, key: impl Into<String>, value: Value) {
        let key = key.into();
        if let Some(i) = self.position(&key) {
            self.entries[i].1 = value;
            return;
        }
        if let Some(n) = array_index(&key) {
            let at = self
                .entries
                .iter()
                .position(|(k, _)| array_index(k).is_none_or(|m| m > n))
                .unwrap_or(self.entries.len());
            self.entries.insert(at, (key, value));
            self.rebuild_index();
            return;
        }
        if let Some(index) = &mut self.index {
            index.insert(key.clone(), self.entries.len() as u32);
        }
        self.entries.push((key, value));
        if self.index.is_none() && self.entries.len() > INDEX_THRESHOLD {
            self.rebuild_index();
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let i = self.position(key)?;
        let (_, v) = self.entries.remove(i);
        self.rebuild_index();
        Some(v)
    }

    fn rebuild_index(&mut self) {
        self.index = (self.entries.len() > INDEX_THRESHOLD).then(|| {
            self.entries.iter().enumerate().map(|(i, (k, _))| (k.clone(), i as u32)).collect()
        });
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut Value)> {
        self.entries.iter_mut().map(|(k, v)| (k.as_str(), v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.entries.iter().map(|(_, v)| v)
    }
}

impl FromIterator<(String, Value)> for Object {
    fn from_iter<T: IntoIterator<Item = (String, Value)>>(iter: T) -> Self {
        let mut o = Object::new();
        for (k, v) in iter {
            o.insert(k, v);
        }
        o
    }
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// `value[key]` for objects; `None` for everything else.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object()?.get(key)
    }

    /// `path.reduce((acc, key) => acc?.[key], value)`, with array indexes as strings.
    pub fn at_path<S: AsRef<str>>(&self, path: &[S]) -> Option<&Value> {
        let mut cur = self;
        for key in path {
            cur = match cur {
                Value::Object(o) => o.get(key.as_ref())?,
                Value::Array(a) => a.get(array_index(key.as_ref())? as usize)?,
                _ => return None,
            };
        }
        Some(cur)
    }

    /// JS truthiness.
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0 && !n.is_nan(),
            Value::String(s) => !s.is_empty(),
            Value::Array(_) | Value::Object(_) => true,
        }
    }

    /// `typeof value`.
    pub fn type_of(&self) -> &'static str {
        match self {
            Value::Null | Value::Array(_) | Value::Object(_) => "object",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
        }
    }

    /// `JSON.stringify(value)`.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out, 0, 0);
        out
    }

    /// `JSON.stringify(value, null, indent)`.
    pub fn to_json_pretty(&self, indent: usize) -> String {
        let mut out = String::new();
        self.write_json(&mut out, indent, 0);
        out
    }

    fn write_json(&self, out: &mut String, indent: usize, depth: usize) {
        let newline = |out: &mut String, depth: usize| {
            if indent > 0 {
                out.push('\n');
                for _ in 0..indent * depth {
                    out.push(' ');
                }
            }
        };
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => {
                if n.is_finite() {
                    out.push_str(&js::number_to_string(*n));
                } else {
                    out.push_str("null");
                }
            }
            Value::String(s) => js::quote_into(s, out),
            Value::Array(a) => {
                if a.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, depth + 1);
                    v.write_json(out, indent, depth + 1);
                }
                newline(out, depth);
                out.push(']');
            }
            Value::Object(o) => {
                if o.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (i, (k, v)) in o.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, depth + 1);
                    js::quote_into(k, out);
                    out.push(':');
                    if indent > 0 {
                        out.push(' ');
                    }
                    v.write_json(out, indent, depth + 1);
                }
                newline(out, depth);
                out.push('}');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_key_order_is_js_order() {
        let mut o = Object::new();
        o.insert("b", Value::Null);
        o.insert("2", Value::Null);
        o.insert("a", Value::Null);
        o.insert("1", Value::Null);
        o.insert("b", Value::Bool(true));
        o.insert("01", Value::Null);
        assert_eq!(o.keys().collect::<Vec<_>>(), ["1", "2", "b", "a", "01"]);
        assert_eq!(o.get("b"), Some(&Value::Bool(true)));
    }

    #[test]
    fn big_objects_stay_consistent() {
        let mut o = Object::new();
        for i in 0..100 {
            o.insert(format!("k{i}"), Value::Number(f64::from(i)));
        }
        o.remove("k50");
        o.insert("7", Value::Null);
        assert_eq!(o.get("k99"), Some(&Value::Number(99.0)));
        assert_eq!(o.get("k50"), None);
        assert_eq!(o.keys().next(), Some("7"));
    }
}
