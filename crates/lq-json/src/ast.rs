//! theme-check's JSON AST (`jsonc/parse.ts`, `jsonc/types.ts`, `json.ts`).

use crate::js;
use crate::jsonc::{parse_tree, ParseError, ParseOptions, TreeKind, TreeNode};
use crate::value::{Object, Value};
use std::borrow::Cow;

/// Byte offsets. Upstream's `loc.start.offset` / `loc.end.offset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Loc {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Literal<'s> {
    String(Cow<'s, str>),
    Number(f64),
    Bool(bool),
    Null,
}

/// `JSONNode`. `Object` children are always `Property`; a `Property`'s `key` is
/// always `Identifier`.
#[derive(Clone, Debug)]
pub enum JsonNode<'s> {
    Object { children: Vec<JsonNode<'s>>, loc: Loc },
    Array { children: Vec<JsonNode<'s>>, loc: Loc },
    Property { key: Box<JsonNode<'s>>, value: Box<JsonNode<'s>>, loc: Loc },
    Identifier { value: Cow<'s, str>, loc: Loc },
    Literal { value: Literal<'s>, loc: Loc },
}

/// `JSONCParseErrors`: message `Failed to parse JSONC`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonParseFailure {
    pub errors: Vec<ParseError>,
}

impl JsonParseFailure {
    pub const MESSAGE: &'static str = "Failed to parse JSONC";
}

/// `toJSONNode(source)`.
pub fn to_json_node(source: &str) -> Result<JsonNode<'_>, JsonParseFailure> {
    let (tree, errors) = parse_tree(
        source,
        ParseOptions { allow_trailing_comma: true, disallow_comments: false, allow_empty_content: false },
    );
    match tree {
        Some(tree) if errors.is_empty() => Ok(convert(tree)),
        _ => Err(JsonParseFailure { errors }),
    }
}

fn loc(n: &TreeNode<'_>) -> Loc {
    Loc { start: n.offset, end: n.offset + n.length }
}

fn convert(n: TreeNode<'_>) -> JsonNode<'_> {
    let l = loc(&n);
    match n.kind {
        TreeKind::Object => {
            JsonNode::Object { children: n.children.into_iter().map(convert).collect(), loc: l }
        }
        TreeKind::Array => {
            JsonNode::Array { children: n.children.into_iter().map(convert).collect(), loc: l }
        }
        TreeKind::Property => {
            let mut it = n.children.into_iter();
            let key = it.next().expect("property key");
            let key_loc = loc(&key);
            let key_value = match key.kind {
                TreeKind::String(s) => s,
                _ => Cow::Borrowed(""),
            };
            let value = it.next().expect("no errors means every property has a value");
            JsonNode::Property {
                key: Box::new(JsonNode::Identifier { value: key_value, loc: key_loc }),
                value: Box::new(convert(value)),
                loc: l,
            }
        }
        TreeKind::String(s) => JsonNode::Literal { value: Literal::String(s), loc: l },
        TreeKind::Number(v) => JsonNode::Literal { value: Literal::Number(v), loc: l },
        TreeKind::Bool(v) => JsonNode::Literal { value: Literal::Bool(v), loc: l },
        TreeKind::Null => JsonNode::Literal { value: Literal::Null, loc: l },
    }
}

impl<'s> JsonNode<'s> {
    pub fn loc(&self) -> Loc {
        match self {
            JsonNode::Object { loc, .. }
            | JsonNode::Array { loc, .. }
            | JsonNode::Property { loc, .. }
            | JsonNode::Identifier { loc, .. }
            | JsonNode::Literal { loc, .. } => *loc,
        }
    }

    /// The upstream `type` string.
    pub fn node_type(&self) -> &'static str {
        match self {
            JsonNode::Object { .. } => "Object",
            JsonNode::Array { .. } => "Array",
            JsonNode::Property { .. } => "Property",
            JsonNode::Identifier { .. } => "Identifier",
            JsonNode::Literal { .. } => "Literal",
        }
    }

    /// `node.raw` on `Identifier` and `Literal`: `JSON.stringify(node.value)`.
    pub fn raw(&self) -> Option<String> {
        match self {
            JsonNode::Identifier { value, .. } => Some(js::quote(value)),
            JsonNode::Literal { value, .. } => Some(match value {
                Literal::String(s) => js::quote(s),
                Literal::Number(n) => js::number_to_string(*n),
                Literal::Bool(b) => b.to_string(),
                Literal::Null => "null".to_string(),
            }),
            _ => None,
        }
    }

    /// The key of a `Property`.
    pub fn property_key(&self) -> Option<&str> {
        match self {
            JsonNode::Property { key, .. } => match &**key {
                JsonNode::Identifier { value, .. } => Some(value),
                _ => None,
            },
            _ => None,
        }
    }

    /// The value of a `Property`.
    pub fn property_value(&self) -> Option<&JsonNode<'s>> {
        match self {
            JsonNode::Property { value, .. } => Some(value),
            _ => None,
        }
    }

    pub fn children(&self) -> &[JsonNode<'s>] {
        match self {
            JsonNode::Object { children, .. } | JsonNode::Array { children, .. } => children,
            _ => &[],
        }
    }

    /// The JS value this node denotes. Later duplicate keys win, in the first's slot.
    pub fn to_value(&self) -> Value {
        match self {
            JsonNode::Object { children, .. } => {
                let mut o = Object::new();
                for c in children {
                    if let (Some(k), Some(v)) = (c.property_key(), c.property_value()) {
                        if k != "__proto__" {
                            o.insert(k, v.to_value());
                        }
                    }
                }
                Value::Object(o)
            }
            JsonNode::Array { children, .. } => {
                Value::Array(children.iter().map(JsonNode::to_value).collect())
            }
            JsonNode::Literal { value, .. } => match value {
                Literal::String(s) => Value::String(s.to_string()),
                Literal::Number(n) => Value::Number(*n),
                Literal::Bool(b) => Value::Bool(*b),
                Literal::Null => Value::Null,
            },
            JsonNode::Property { .. } | JsonNode::Identifier { .. } => Value::Null,
        }
    }
}

/// A key in `nodeAtPath`: a string against objects, an index against arrays.
#[derive(Clone, Copy, Debug)]
pub enum PathKey<'a> {
    Str(&'a str),
    Index(usize),
}

impl<'a> From<&'a str> for PathKey<'a> {
    fn from(s: &'a str) -> Self {
        PathKey::Str(s)
    }
}

impl<'a> From<&'a String> for PathKey<'a> {
    fn from(s: &'a String) -> Self {
        PathKey::Str(s)
    }
}

impl From<usize> for PathKey<'_> {
    fn from(i: usize) -> Self {
        PathKey::Index(i)
    }
}

/// `nodeAtPath(node, path)`. Mirrors JS: a numeric string indexes arrays, the first
/// matching property wins on duplicate keys, and a number never matches a key.
pub fn node_at_path<'n, 's, K>(node: &'n JsonNode<'s>, path: &[K]) -> Option<&'n JsonNode<'s>>
where
    K: Copy,
    PathKey<'n>: From<K>,
{
    let mut cur = node;
    for key in path {
        let key = PathKey::from(*key);
        cur = match cur {
            JsonNode::Object { children, .. } => match key {
                PathKey::Str(k) => {
                    children.iter().find(|c| c.property_key() == Some(k))?.property_value()?
                }
                PathKey::Index(_) => return None,
            },
            JsonNode::Array { children, .. } => {
                let i = match key {
                    PathKey::Index(i) => i,
                    PathKey::Str(k) => crate::value::array_index(k)? as usize,
                };
                children.get(i)?
            }
            _ => return None,
        };
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_like_upstream_spec() {
        let ast = to_json_node(r#"{"key": "value"}"#).unwrap();
        assert_eq!(ast.loc(), Loc { start: 0, end: 16 });
        let p = &ast.children()[0];
        assert_eq!(p.loc(), Loc { start: 1, end: 15 });
        assert_eq!(p.property_key(), Some("key"));
        assert_eq!(p.property_value().unwrap().loc(), Loc { start: 8, end: 15 });

        let a = to_json_node(r#"["one", true, null, 10]"#).unwrap();
        assert_eq!(a.children()[3].loc(), Loc { start: 20, end: 22 });
        assert_eq!(a.children()[3].raw().as_deref(), Some("10"));

        let c = to_json_node("/** block comment */10").unwrap();
        assert_eq!(c.loc(), Loc { start: 20, end: 22 });

        let t = to_json_node("{\n  \"key\": \"value\",\n}").unwrap();
        assert_eq!(t.loc().end, 21);
        assert_eq!(t.children()[0].loc().end as usize, "{\n  \"key\": \"value\"".len());
    }

    #[test]
    fn errors() {
        let e = to_json_node("{").unwrap_err();
        assert_eq!(e.errors[0].error, crate::jsonc::ParseErrorCode::CloseBraceExpected);
        assert!(to_json_node("").is_err());
    }

    #[test]
    fn node_at_path_works() {
        let n = to_json_node(r#"{"a":[{"b":1}],"a":2}"#).unwrap();
        let p: Vec<&str> = vec!["a", "0", "b"];
        assert!(node_at_path(&n, &p).is_some());
    }
}
