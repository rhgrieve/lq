//! Liquid values. Objects carry a Shopify type name (`product`, `collection`, ...) so the cost
//! model can tell a database-backed property from a plain field, and an id so repeated reads of
//! the same object can be priced as cached.

use indexmap::IndexMap;
use std::cell::Cell;
use std::fmt::Write as _;
use std::rc::Rc;

thread_local! {
    static NEXT_ID: Cell<u32> = const { Cell::new(1) };
}

fn next_id() -> u32 {
    NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id.wrapping_add(1));
        id
    })
}

#[derive(Debug)]
pub struct Object {
    pub id: u32,
    /// The Shopify object type, such as `product`. Empty for plain hashes.
    pub ty: Rc<str>,
    pub fields: IndexMap<Rc<str>, Value>,
}

impl Object {
    pub fn new(ty: &str) -> Self {
        Object { id: next_id(), ty: Rc::from(ty), fields: IndexMap::new() }
    }

    pub fn with(ty: &str, fields: IndexMap<Rc<str>, Value>) -> Self {
        Object { id: next_id(), ty: Rc::from(ty), fields }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields.get(key)
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.fields.insert(Rc::from(key), value);
    }
}

#[derive(Clone, Debug, Default)]
pub enum Value {
    #[default]
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Array(Rc<Vec<Value>>),
    Object(Rc<Object>),
    Range(i64, i64),
    /// The `empty` literal.
    Empty,
    /// The `blank` literal.
    Blank,
}

impl Value {
    pub fn str(s: impl AsRef<str>) -> Value {
        Value::Str(Rc::from(s.as_ref()))
    }

    pub fn array(v: Vec<Value>) -> Value {
        Value::Array(Rc::new(v))
    }

    pub fn object(o: Object) -> Value {
        Value::Object(Rc::new(o))
    }

    pub fn is_truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    /// Ruby's `empty?`, used for `== empty`.
    pub fn is_empty(&self) -> bool {
        match self {
            Value::Str(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.fields.is_empty(),
            Value::Range(a, b) => b < a,
            Value::Empty | Value::Blank => true,
            _ => false,
        }
    }

    /// Rails' `blank?`, used for `== blank`.
    pub fn is_blank(&self) -> bool {
        match self {
            Value::Nil | Value::Bool(false) => true,
            Value::Str(s) => s.trim().is_empty(),
            _ => self.is_empty(),
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            Value::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    /// Liquid's number coercion for math filters: strings parse, anything else is 0.
    pub fn to_number(&self) -> Value {
        match self {
            Value::Int(_) | Value::Float(_) => self.clone(),
            Value::Str(s) => {
                let t = s.trim();
                if let Ok(i) = t.parse::<i64>() {
                    Value::Int(i)
                } else if let Ok(f) = t.parse::<f64>() {
                    Value::Float(f)
                } else {
                    Value::Int(0)
                }
            }
            _ => Value::Int(0),
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self.to_number() {
            Value::Int(i) => Some(i),
            Value::Float(f) => Some(f as i64),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Value::Str(s) => s.chars().count(),
            Value::Array(a) => a.len(),
            Value::Object(o) => o.fields.len(),
            Value::Range(a, b) => (b - a + 1).max(0) as usize,
            _ => 0,
        }
    }

    /// The items a `for` loop iterates.
    pub fn to_items(&self) -> Vec<Value> {
        match self {
            Value::Array(a) => a.as_ref().clone(),
            Value::Range(a, b) => (*a..=*b).map(Value::Int).collect(),
            // A hash iterates as [key, value] pairs.
            Value::Object(o) if o.ty.is_empty() || &*o.ty == "settings" => o
                .fields
                .iter()
                .map(|(k, v)| Value::array(vec![Value::Str(k.clone()), v.clone()]))
                .collect(),
            // Drops with a natural list iterate it (`for block in section.blocks`).
            Value::Object(o) => match o.get("__items") {
                Some(Value::Array(a)) => a.as_ref().clone(),
                _ => Vec::new(),
            },
            Value::Str(s) if !s.is_empty() => vec![self.clone()],
            _ => Vec::new(),
        }
    }

    pub fn type_name(&self) -> &str {
        match self {
            Value::Object(o) => &o.ty,
            Value::Array(_) => "array",
            Value::Str(_) => "string",
            _ => "",
        }
    }

    /// What `{{ value }}` prints.
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.render_into(&mut out);
        out
    }

    pub fn render_into(&self, out: &mut String) {
        match self {
            Value::Nil | Value::Empty | Value::Blank => {}
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Int(i) => {
                let _ = write!(out, "{i}");
            }
            Value::Float(f) => out.push_str(&format_float(*f)),
            Value::Str(s) => out.push_str(s),
            Value::Array(a) => {
                for v in a.iter() {
                    v.render_into(out);
                }
            }
            Value::Range(a, b) => {
                let _ = write!(out, "{a}..{b}");
            }
            Value::Object(o) => {
                if let Some(v) = o.get("__to_s") {
                    v.render_into(out);
                }
            }
        }
    }

    /// Liquid's `==`.
    pub fn liquid_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Empty, v) | (v, Value::Empty) => v.is_empty(),
            (Value::Blank, v) | (v, Value::Blank) => v.is_blank(),
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (a, b) if a.is_number() && b.is_number() => a.as_f64() == b.as_f64(),
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.liquid_eq(y))
            }
            (Value::Object(a), Value::Object(b)) => Rc::ptr_eq(a, b) || a.id == b.id,
            (Value::Object(o), Value::Str(s)) | (Value::Str(s), Value::Object(o)) => {
                o.get("__to_s").is_some_and(|v| matches!(v, Value::Str(t) if t == s))
            }
            _ => false,
        }
    }

    pub fn is_number(&self) -> bool {
        matches!(self, Value::Int(_) | Value::Float(_))
    }

    pub fn compare(&self, other: &Value) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (a, b) if a.is_number() && b.is_number() => a.as_f64()?.partial_cmp(&b.as_f64()?),
            (Value::Str(a), Value::Str(b)) => Some(a.cmp(b)),
            _ => None,
        }
    }

    pub fn contains(&self, needle: &Value) -> bool {
        match self {
            Value::Str(s) => s.contains(&*needle.render()),
            Value::Array(a) => a.iter().any(|v| v.liquid_eq(needle)),
            Value::Object(o) => o.fields.contains_key(&*needle.render()),
            _ => false,
        }
    }
}

/// Ruby's `Float#to_s` for the values themes print: `1.0`, `0.5`, `12.25`.
pub fn format_float(f: f64) -> String {
    if f.is_finite() && f == f.trunc() && f.abs() < 1e16 {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}

/// Converts lq-json's value into a Liquid value, typing objects that carry a `__type` key.
pub fn from_json(v: &lq_json::Value) -> Value {
    match v {
        lq_json::Value::Null => Value::Nil,
        lq_json::Value::Bool(b) => Value::Bool(*b),
        lq_json::Value::Number(n) => {
            if n.fract() == 0.0 && n.abs() < 9e15 {
                Value::Int(*n as i64)
            } else {
                Value::Float(*n)
            }
        }
        lq_json::Value::String(s) => Value::str(s),
        lq_json::Value::Array(a) => Value::array(a.iter().map(from_json).collect()),
        lq_json::Value::Object(o) => {
            let ty = o.get("__type").and_then(|t| t.as_str()).unwrap_or("");
            let mut obj = Object::new(ty);
            for (k, v) in o.iter() {
                if k != "__type" {
                    obj.set(k, from_json(v));
                }
            }
            Value::object(obj)
        }
    }
}

/// Converts a Liquid value to JSON for the `json` filter and for writing mock shops.
pub fn to_json(v: &Value, out: &mut String, depth: usize) {
    if depth > 8 {
        out.push_str("null");
        return;
    }
    match v {
        Value::Nil | Value::Empty | Value::Blank => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::Float(f) => {
            let _ = write!(out, "{f}");
        }
        Value::Str(s) => {
            out.push_str(&serde_json::to_string(&**s).unwrap_or_default());
        }
        Value::Range(a, b) => {
            let _ = write!(out, "\"{a}..{b}\"");
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                to_json(x, out, depth + 1);
            }
            out.push(']');
        }
        Value::Object(o) => {
            out.push('{');
            let mut first = true;
            for (k, x) in o.fields.iter() {
                if k.starts_with("__") {
                    continue;
                }
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str(&serde_json::to_string(&**k).unwrap_or_default());
                out.push(':');
                to_json(x, out, depth + 1);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_print_like_ruby() {
        assert_eq!(format_float(1.0), "1.0");
        assert_eq!(format_float(0.5), "0.5");
        assert_eq!(Value::Int(3).render(), "3");
    }

    #[test]
    fn empty_and_blank() {
        assert!(Value::str("").liquid_eq(&Value::Empty));
        assert!(Value::str("  ").liquid_eq(&Value::Blank));
        assert!(!Value::str("a").liquid_eq(&Value::Empty));
        assert!(Value::Nil.liquid_eq(&Value::Blank));
    }
}
