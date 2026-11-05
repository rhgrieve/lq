//! Helpers shared by the schema checks: `{% schema %}` access, JS semantics over the
//! dynamic `validSchema` value, a model of `Promise.all` ordering, and the theme-wide
//! CSS classes of `ValidScopedCSSClass`.

use crate::api::Cx;
use crate::to_schema::ThemeSchema;
use lq_json::validator::JsError;
use lq_json::{JsonNode, Value};
use lq_syntax::{LiquidRawTag, Node};
use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

/// The result of something that can throw in JS.
pub type Js<T> = Result<T, JsError>;

pub fn type_error(message: &str) -> JsError {
    JsError { name: "TypeError".into(), message: message.into() }
}

/// `node.name === 'schema' && node.body.kind === 'json'`.
pub fn schema_raw_tag<'n, 's>(node: &'n Node<'s>) -> Option<&'n LiquidRawTag<'s>> {
    let Node::LiquidRawTag(tag) = node else { return None };
    if tag.name != "schema" {
        return None;
    }
    match &tag.body {
        Node::RawMarkup(b) if b.kind == lq_syntax::RawMarkupKind::Json => Some(tag),
        _ => None,
    }
}

/// The value of `getSchema(context)`, after upstream's usual early returns.
pub struct SchemaParts<'a> {
    pub schema: &'a ThemeSchema<'a>,
    pub valid: &'a Value,
    pub ast: &'a JsonNode<'a>,
}

/// `getSchema(context)` with `if (!validSchema || validSchema instanceof Error) return;`
/// and `if (!ast || ast instanceof Error) return;`.
pub fn valid_parts<'a, C>(cx: &Cx<'a, '_, C>) -> Option<SchemaParts<'a>> {
    let schema = cx.schema()?;
    let valid = schema.valid_value()?;
    let ast = schema.ast.as_ref().ok()?;
    Some(SchemaParts { schema, valid, ast })
}

/// `obj.key`. Throws on `null`; primitives have no JSON-visible properties.
pub fn prop<'v>(v: &'v Value, key: &str) -> Js<Option<&'v Value>> {
    match v {
        Value::Null => Err(type_error("Cannot read properties of null")),
        _ => Ok(v.get(key)),
    }
}

/// `'key' in obj`. Throws on primitives.
pub fn in_op(v: &Value, key: &str) -> Js<bool> {
    match v {
        Value::Object(o) => Ok(o.contains_key(key)),
        Value::Array(a) => Ok(key == "length" || lq_json::value::array_index(key).is_some_and(|i| (i as usize) < a.len())),
        _ => Err(type_error("Cannot use 'in' operator on a primitive")),
    }
}

/// `x ?? fallback`, as an `Option`.
pub fn nullish(v: Option<&Value>) -> Option<&Value> {
    v.filter(|v| !v.is_null())
}

/// A value on which upstream calls array methods. Other values throw.
pub fn array_of(v: &Value) -> Js<&[Value]> {
    v.as_array().ok_or_else(|| type_error("not an array"))
}

/// `Object.entries(v)`. Characters of strings aren't produced: no caller can use them.
pub fn entries(v: &Value) -> Vec<(Cow<'_, str>, &Value)> {
    match v {
        Value::Object(o) => o.iter().map(|(k, v)| (Cow::Borrowed(k), v)).collect(),
        Value::Array(a) => a.iter().enumerate().map(|(i, v)| (Cow::Owned(i.to_string()), v)).collect(),
        _ => Vec::new(),
    }
}

/// `a === b` on JSON values; objects and arrays are distinct.
pub fn strict_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Value::Object(_) | Value::Array(_)), _) | (_, Some(Value::Object(_) | Value::Array(_))) => false,
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// `` `${v}` ``.
pub fn js_string(v: Option<&Value>) -> Cow<'_, str> {
    match v {
        None => Cow::Borrowed("undefined"),
        Some(Value::Null) => Cow::Borrowed("null"),
        Some(Value::Bool(b)) => Cow::Borrowed(if *b { "true" } else { "false" }),
        Some(Value::Number(n)) => Cow::Owned(lq_json::js::number_to_string(*n)),
        Some(Value::String(s)) => Cow::Borrowed(s),
        Some(Value::Object(_)) => Cow::Borrowed("[object Object]"),
        Some(Value::Array(a)) => {
            let parts: Vec<Cow<'_, str>> = a
                .iter()
                .map(|x| if x.is_null() { Cow::Borrowed("") } else { js_string(Some(x)) })
                .collect();
            Cow::Owned(parts.join(","))
        }
    }
}

/// Own and inherited members of `Object.prototype`.
pub const OBJECT_PROTOTYPE_KEYS: [&str; 12] = [
    "__defineGetter__", "__defineSetter__", "__lookupGetter__", "__lookupSetter__", "__proto__", "constructor",
    "hasOwnProperty", "isPrototypeOf", "propertyIsEnumerable", "toLocaleString", "toString", "valueOf",
];

const ARRAY_PROTOTYPE_KEYS: [&str; 39] = [
    "at", "concat", "copyWithin", "entries", "every", "fill", "filter", "find", "findIndex", "findLast",
    "findLastIndex", "flat", "flatMap", "forEach", "includes", "indexOf", "join", "keys", "lastIndexOf", "map",
    "pop", "push", "reduce", "reduceRight", "reverse", "shift", "slice", "some", "sort", "splice", "toLocaleString",
    "toReversed", "toSorted", "toSpliced", "toString", "unshift", "values", "with", "constructor",
];

const STRING_PROTOTYPE_KEYS: [&str; 51] = [
    "anchor", "at", "big", "blink", "bold", "charAt", "charCodeAt", "codePointAt", "concat", "endsWith", "fixed",
    "fontcolor", "fontsize", "includes", "indexOf", "isWellFormed", "italics", "lastIndexOf", "link", "localeCompare",
    "match", "matchAll", "normalize", "padEnd", "padStart", "repeat", "replace", "replaceAll", "search", "slice",
    "small", "split", "startsWith", "strike", "sub", "substr", "substring", "sup", "toLocaleLowerCase",
    "toLocaleUpperCase", "toLowerCase", "toString", "toUpperCase", "toWellFormed", "trim", "trimEnd", "trimLeft",
    "trimRight", "trimStart", "valueOf", "constructor",
];

const NUMBER_PROTOTYPE_KEYS: [&str; 4] = ["toExponential", "toFixed", "toPrecision", "constructor"];

const FUNCTION_KEYS: [&str; 8] = ["apply", "arguments", "bind", "call", "caller", "length", "name", "prototype"];

/// What `deepGet(obj, path)` evaluates to.
#[derive(Clone, Debug, PartialEq)]
pub enum DeepValue<'v> {
    Undefined,
    Value(&'v Value),
    /// A character of a string.
    Char(String),
    /// `.length`.
    Number(f64),
    /// A member of a prototype: a function, or the prototype object itself.
    Function,
}

impl DeepValue<'_> {
    /// `translation.length` for a value upstream compares with a threshold; `None`
    /// is `undefined`.
    pub fn js_length(&self) -> Js<Option<f64>> {
        Ok(match self {
            DeepValue::Value(Value::Null) => return Err(type_error("Cannot read properties of null")),
            DeepValue::Value(Value::String(s)) => Some(crate::utils::js_len(s) as f64),
            DeepValue::Value(Value::Array(a)) => Some(a.len() as f64),
            DeepValue::Char(s) => Some(crate::utils::js_len(s) as f64),
            _ => None,
        })
    }

    /// `` `${v}` ``.
    pub fn display(&self) -> Cow<'_, str> {
        match self {
            DeepValue::Value(v) => js_string(Some(v)),
            DeepValue::Char(s) => Cow::Borrowed(s),
            DeepValue::Number(n) => Cow::Owned(lq_json::js::number_to_string(*n)),
            DeepValue::Undefined => Cow::Borrowed("undefined"),
            DeepValue::Function => Cow::Borrowed("function"),
        }
    }
}

/// `deepGet(obj, path)`: `path.reduce((acc, key) => acc?.[key], obj)`, including what JS
/// finds on prototypes: `foo.length` or `foo.0` of a string, `toString` of anything.
///
/// With `own_only` ([`PrototypeLookup`](crate::api::Divergence::PrototypeLookup)) it is `Object.hasOwn` at each step:
/// keys of objects, indices and `length` of arrays and strings, nothing inherited.
pub fn deep_get<'v>(root: Option<&'v Value>, path: &[&str], own_only: bool) -> DeepValue<'v> {
    let mut cur = match root {
        Some(v) => DeepValue::Value(v),
        None => DeepValue::Undefined,
    };
    if own_only {
        for key in path {
            cur = own_member(cur, key);
        }
        return cur;
    }
    for key in path {
        cur = match cur {
            DeepValue::Undefined | DeepValue::Value(Value::Null) => DeepValue::Undefined,
            DeepValue::Value(Value::Object(o)) => match o.get(key) {
                Some(v) => DeepValue::Value(v),
                None if OBJECT_PROTOTYPE_KEYS.contains(key) => DeepValue::Function,
                None => DeepValue::Undefined,
            },
            DeepValue::Value(Value::Array(a)) => {
                if *key == "length" {
                    DeepValue::Number(a.len() as f64)
                } else if let Some(v) = lq_json::value::array_index(key).and_then(|i| a.get(i as usize)) {
                    DeepValue::Value(v)
                } else if ARRAY_PROTOTYPE_KEYS.contains(key) || OBJECT_PROTOTYPE_KEYS.contains(key) {
                    DeepValue::Function
                } else {
                    DeepValue::Undefined
                }
            }
            DeepValue::Value(Value::String(s)) => string_member(s, key),
            DeepValue::Char(s) => string_member(&s, key),
            DeepValue::Value(Value::Number(_)) | DeepValue::Number(_) => member_if(NUMBER_PROTOTYPE_KEYS.contains(key), key),
            DeepValue::Value(Value::Bool(_)) => member_if(false, key),
            DeepValue::Function => member_if(FUNCTION_KEYS.contains(key), key),
        };
    }
    cur
}

fn own_member<'v>(cur: DeepValue<'v>, key: &str) -> DeepValue<'v> {
    match cur {
        DeepValue::Value(Value::Object(o)) => o.get(key).map_or(DeepValue::Undefined, DeepValue::Value),
        DeepValue::Value(Value::Array(a)) if key == "length" => DeepValue::Number(a.len() as f64),
        DeepValue::Value(Value::Array(a)) => lq_json::value::array_index(key)
            .and_then(|i| a.get(i as usize))
            .map_or(DeepValue::Undefined, DeepValue::Value),
        DeepValue::Value(Value::String(s)) => own_string_member(s, key),
        DeepValue::Char(s) => own_string_member(&s, key),
        _ => DeepValue::Undefined,
    }
}

fn own_string_member<'v>(s: &str, key: &str) -> DeepValue<'v> {
    if key == "length" || lq_json::value::array_index(key).is_some() {
        string_member(s, key)
    } else {
        DeepValue::Undefined
    }
}

fn member_if<'v>(found: bool, key: &str) -> DeepValue<'v> {
    if found || OBJECT_PROTOTYPE_KEYS.contains(&key) {
        DeepValue::Function
    } else {
        DeepValue::Undefined
    }
}

fn string_member<'v>(s: &str, key: &str) -> DeepValue<'v> {
    if key == "length" {
        return DeepValue::Number(crate::utils::js_len(s) as f64);
    }
    if let Some(i) = lq_json::value::array_index(key) {
        let units: Vec<u16> = s.encode_utf16().collect();
        return match units.get(i as usize) {
            Some(u) => DeepValue::Char(String::from_utf16_lossy(&[*u])),
            None => DeepValue::Undefined,
        };
    }
    if STRING_PROTOTYPE_KEYS.contains(&key) || OBJECT_PROTOTYPE_KEYS.contains(&key) {
        DeepValue::Function
    } else {
        DeepValue::Undefined
    }
}

/// A model of the JS event loop for code that fans out with `Promise.all`: tasks run by
/// the time they were scheduled for, first scheduled first, so reports come out in the
/// order upstream's awaits produce. File system calls complete in request order, one per
/// event loop turn, with the microtasks they start drained in between.
#[derive(Default)]
pub struct Sched<'t> {
    now: u32,
    seq: u32,
    /// When the next file system call can complete.
    io_free: u32,
    queue: BinaryHeap<Reverse<Scheduled<'t>>>,
}

type Task<'t> = Box<dyn FnOnce(&mut Sched<'t>) + 't>;

struct Scheduled<'t> {
    at: u32,
    seq: u32,
    task: Task<'t>,
}

impl PartialEq for Scheduled<'_> {
    fn eq(&self, other: &Self) -> bool {
        (self.at, self.seq) == (other.at, other.seq)
    }
}
impl Eq for Scheduled<'_> {}
impl PartialOrd for Scheduled<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Scheduled<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.at, self.seq).cmp(&(other.at, other.seq))
    }
}

impl<'t> Sched<'t> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `task` `delay` ticks from now.
    pub fn after(&mut self, delay: u32, task: impl FnOnce(&mut Sched<'t>) + 't) {
        self.seq += 1;
        self.queue.push(Reverse(Scheduled { at: self.now + delay, seq: self.seq, task: Box::new(task) }));
    }

    /// Runs `task` when a file system call started now completes.
    pub fn after_io(&mut self, task: impl FnOnce(&mut Sched<'t>) + 't) {
        const TICKS_PER_TURN: u32 = 1000;
        let at = self.io_free.max(self.now + TICKS_PER_TURN);
        self.io_free = at + TICKS_PER_TURN;
        self.after(at - self.now, task);
    }

    pub fn run(&mut self) {
        while let Some(Reverse(next)) = self.queue.pop() {
            self.now = next.at;
            (next.task)(self);
        }
    }
}

/// The classes `ValidScopedCSSClass` needs from the whole theme, computed once per run.
pub struct ThemeCssClasses {
    /// `getAssetCSSClasses`.
    pub asset: HashSet<String>,
    /// `getAllThemeCSSClasses`.
    pub all: HashSet<String>,
}

/// `DEPRECATED_FONT_HANDLES`, sorted for binary search (1341 handles).
static DEPRECATED_FONT_HANDLES: [&str; 1341] = [
    "agmena_i3", "agmena_i4", "agmena_i6", "agmena_i7", "agmena_n3", "agmena_n4", "agmena_n6", "agmena_n7",
    "akko_i2", "akko_i3", "akko_i4", "akko_i5", "akko_i7", "akko_i9", "akko_n2", "akko_n3", "akko_n4", "akko_n5",
    "akko_n7", "akko_n9", "alfie_n4", "americana_i4", "americana_n4", "americana_n7", "americana_n8",
    "antique_olive_i4", "antique_olive_n3", "antique_olive_n4", "antique_olive_n7", "antique_olive_n9",
    "armata_n4", "avenir_next_i1", "avenir_next_i2", "avenir_next_i3", "avenir_next_i4", "avenir_next_i5",
    "avenir_next_i6", "avenir_next_i7", "avenir_next_i8", "avenir_next_n1", "avenir_next_n2", "avenir_next_n3",
    "avenir_next_n4", "avenir_next_n5", "avenir_next_n6", "avenir_next_n7", "avenir_next_n8",
    "avenir_next_rounded_i4", "avenir_next_rounded_i5", "avenir_next_rounded_i6", "avenir_next_rounded_i7",
    "avenir_next_rounded_n4", "avenir_next_rounded_n5", "avenir_next_rounded_n6", "avenir_next_rounded_n7",
    "azbuka_i3", "azbuka_i4", "azbuka_i5", "azbuka_i6", "azbuka_i7", "azbuka_n2", "azbuka_n3", "azbuka_n4",
    "azbuka_n5", "azbuka_n6", "azbuka_n7", "azbuka_n8", "azbuka_n9", "basic_commercial_i3", "basic_commercial_i4",
    "basic_commercial_i7", "basic_commercial_i8", "basic_commercial_n3", "basic_commercial_n4",
    "basic_commercial_n7", "basic_commercial_n8", "basic_commercial_soft_rounded_n4",
    "basic_commercial_soft_rounded_n7", "basic_commercial_soft_rounded_n9", "baskerville_no_2_i4",
    "baskerville_no_2_i7", "baskerville_no_2_n4", "baskerville_no_2_n7", "bauer_bodoni_i4", "bauer_bodoni_i7",
    "bauer_bodoni_i9", "bauer_bodoni_n4", "bauer_bodoni_n7", "bauer_bodoni_n9", "beefcakes_i4", "beefcakes_n4",
    "bembo_book_i4", "bembo_book_i7", "bembo_book_n4", "bembo_book_n7", "bernhard_modern_i4",
    "bernhard_modern_i7", "bernhard_modern_n4", "bernhard_modern_n7", "bio_rhyme_n2", "bio_rhyme_n3",
    "bio_rhyme_n4", "bio_rhyme_n5", "bio_rhyme_n6", "bio_rhyme_n7", "bio_rhyme_n8", "bodoni_poster_i9",
    "bodoni_poster_n9", "burlingame_i1", "burlingame_i2", "burlingame_i3", "burlingame_i4", "burlingame_i5",
    "burlingame_i6", "burlingame_i7", "burlingame_i8", "burlingame_i9", "burlingame_n1", "burlingame_n2",
    "burlingame_n3", "burlingame_n4", "burlingame_n5", "burlingame_n6", "burlingame_n7", "burlingame_n8",
    "burlingame_n9", "cachet_i1", "cachet_i2", "cachet_i3", "cachet_i4", "cachet_i5", "cachet_i6", "cachet_i7",
    "cachet_i8", "cachet_n1", "cachet_n2", "cachet_n3", "cachet_n4", "cachet_n5", "cachet_n6", "cachet_n7",
    "cachet_n8", "cardamon_i4", "cardamon_i5", "cardamon_i6", "cardamon_i7", "cardamon_n4", "cardamon_n5",
    "cardamon_n6", "cardamon_n7", "carter_sans_i4", "carter_sans_i5", "carter_sans_i6", "carter_sans_i7",
    "carter_sans_n4", "carter_sans_n5", "carter_sans_n6", "carter_sans_n7", "caslon_bold_i4", "caslon_bold_n4",
    "caslon_old_face_i4", "caslon_old_face_n4", "caslon_old_face_n7", "centaur_i4", "centaur_i7", "centaur_n4",
    "centaur_n7", "century_gothic_i4", "century_gothic_i7", "century_gothic_n4", "century_gothic_n7",
    "chong_modern_i3", "chong_modern_i4", "chong_modern_i7", "chong_modern_n3", "chong_modern_n4",
    "chong_modern_n7", "claire_news_n3", "claire_news_n7", "cooper_bt_i2", "cooper_bt_i5", "cooper_bt_i7",
    "cooper_bt_i9", "cooper_bt_n2", "cooper_bt_n5", "cooper_bt_n7", "cooper_bt_n9", "daytona_i2", "daytona_i3",
    "daytona_i4", "daytona_i6", "daytona_i7", "daytona_n2", "daytona_n3", "daytona_n4", "daytona_n6",
    "daytona_n7", "din_neuzeit_grotesk_n3", "din_neuzeit_grotesk_n7", "din_next_i2", "din_next_i3", "din_next_i4",
    "din_next_i5", "din_next_i7", "din_next_i8", "din_next_i9", "din_next_n2", "din_next_n3", "din_next_n4",
    "din_next_n5", "din_next_n7", "din_next_n8", "din_next_n9", "din_next_slab_i2", "din_next_slab_i3",
    "din_next_slab_i4", "din_next_slab_i5", "din_next_slab_i7", "din_next_slab_i8", "din_next_slab_i9",
    "din_next_slab_n2", "din_next_slab_n3", "din_next_slab_n4", "din_next_slab_n5", "din_next_slab_n7",
    "din_next_slab_n8", "din_next_slab_n9", "electra_i4", "electra_i7", "electra_n4", "electra_n7",
    "eurostile_next_i2", "eurostile_next_i3", "eurostile_next_i4", "eurostile_next_i6", "eurostile_next_i7",
    "eurostile_next_n2", "eurostile_next_n3", "eurostile_next_n4", "eurostile_next_n6", "eurostile_next_n7",
    "fette_gotisch_n4", "ff_meta_i1", "ff_meta_i2", "ff_meta_i3", "ff_meta_i4", "ff_meta_i5", "ff_meta_i6",
    "ff_meta_i7", "ff_meta_i9", "ff_meta_n1", "ff_meta_n2", "ff_meta_n3", "ff_meta_n4", "ff_meta_n5",
    "ff_meta_n6", "ff_meta_n7", "ff_meta_n9", "ff_meta_serif_i3", "ff_meta_serif_i4", "ff_meta_serif_i5",
    "ff_meta_serif_i7", "ff_meta_serif_i8", "ff_meta_serif_i9", "ff_meta_serif_n3", "ff_meta_serif_n4",
    "ff_meta_serif_n5", "ff_meta_serif_n7", "ff_meta_serif_n8", "ff_meta_serif_n9", "ff_tisa_i2", "ff_tisa_i3",
    "ff_tisa_i4", "ff_tisa_i5", "ff_tisa_i7", "ff_tisa_i8", "ff_tisa_i9", "ff_tisa_n2", "ff_tisa_n3",
    "ff_tisa_n4", "ff_tisa_n5", "ff_tisa_n7", "ff_tisa_n8", "ff_tisa_n9", "ff_tisa_sans_i2", "ff_tisa_sans_i3",
    "ff_tisa_sans_i4", "ff_tisa_sans_i5", "ff_tisa_sans_i7", "ff_tisa_sans_i8", "ff_tisa_sans_i9",
    "ff_tisa_sans_n2", "ff_tisa_sans_n3", "ff_tisa_sans_n4", "ff_tisa_sans_n5", "ff_tisa_sans_n7",
    "ff_tisa_sans_n8", "ff_tisa_sans_n9", "ff_unit_i2", "ff_unit_i3", "ff_unit_i4", "ff_unit_i5", "ff_unit_i7",
    "ff_unit_i8", "ff_unit_i9", "ff_unit_n2", "ff_unit_n3", "ff_unit_n4", "ff_unit_n5", "ff_unit_n7",
    "ff_unit_n8", "ff_unit_n9", "ff_unit_rounded_i3", "ff_unit_rounded_i4", "ff_unit_rounded_i5",
    "ff_unit_rounded_i7", "ff_unit_rounded_i8", "ff_unit_rounded_i9", "ff_unit_rounded_n3", "ff_unit_rounded_n4",
    "ff_unit_rounded_n5", "ff_unit_rounded_n7", "ff_unit_rounded_n8", "ff_unit_rounded_n9", "ff_unit_slab_i2",
    "ff_unit_slab_i3", "ff_unit_slab_i4", "ff_unit_slab_i5", "ff_unit_slab_i7", "ff_unit_slab_i8",
    "ff_unit_slab_i9", "ff_unit_slab_n2", "ff_unit_slab_n3", "ff_unit_slab_n4", "ff_unit_slab_n5",
    "ff_unit_slab_n7", "ff_unit_slab_n8", "ff_unit_slab_n9", "friz_quadrata_i4", "friz_quadrata_i7",
    "friz_quadrata_n4", "friz_quadrata_n7", "frutiger_serif_i3", "frutiger_serif_i4", "frutiger_serif_i5",
    "frutiger_serif_i7", "frutiger_serif_i8", "frutiger_serif_n3", "frutiger_serif_n4", "frutiger_serif_n5",
    "frutiger_serif_n7", "frutiger_serif_n8", "fs_kim_i4", "fs_kim_i5", "fs_kim_i7", "fs_kim_i8", "fs_kim_i9",
    "fs_kim_n4", "fs_kim_n5", "fs_kim_n7", "fs_kim_n8", "fs_kim_n9", "fs_koopman_i1", "fs_koopman_i3",
    "fs_koopman_i4", "fs_koopman_i5", "fs_koopman_i7", "fs_koopman_i8", "fs_koopman_i9", "fs_koopman_n1",
    "fs_koopman_n3", "fs_koopman_n4", "fs_koopman_n5", "fs_koopman_n7", "fs_koopman_n8", "fs_koopman_n9",
    "fs_siena_i1", "fs_siena_i2", "fs_siena_i3", "fs_siena_i4", "fs_siena_i5", "fs_siena_i6", "fs_siena_i7",
    "fs_siena_n1", "fs_siena_n2", "fs_siena_n3", "fs_siena_n4", "fs_siena_n5", "fs_siena_n6", "fs_siena_n7",
    "futura_black_n4", "futura_n3", "futura_n4", "futura_n5", "futura_n6", "futura_n7", "futura_n8", "futura_n9",
    "futura_o3", "futura_o4", "futura_o5", "futura_o6", "futura_o7", "futura_o8", "geometric_415_i4",
    "geometric_415_i6", "geometric_415_i7", "geometric_415_n4", "geometric_415_n6", "geometric_415_n7",
    "georgia_pro_i3", "georgia_pro_i4", "georgia_pro_i6", "georgia_pro_i7", "georgia_pro_i9", "georgia_pro_n3",
    "georgia_pro_n4", "georgia_pro_n6", "georgia_pro_n7", "georgia_pro_n9", "gill_sans_nova_i1",
    "gill_sans_nova_i2", "gill_sans_nova_i3", "gill_sans_nova_i4", "gill_sans_nova_i5", "gill_sans_nova_i6",
    "gill_sans_nova_i7", "gill_sans_nova_n1", "gill_sans_nova_n2", "gill_sans_nova_n3", "gill_sans_nova_n4",
    "gill_sans_nova_n5", "gill_sans_nova_n6", "gill_sans_nova_n7", "gill_sans_nova_n8", "gill_sans_nova_n9",
    "glegoo_n4", "glegoo_n7", "goudy_old_style_i4", "goudy_old_style_i7", "goudy_old_style_n4",
    "goudy_old_style_n7", "goudy_old_style_n8", "harmonia_sans_i3", "harmonia_sans_i4", "harmonia_sans_i6",
    "harmonia_sans_i7", "harmonia_sans_i9", "harmonia_sans_n3", "harmonia_sans_n4", "harmonia_sans_n6",
    "harmonia_sans_n7", "harmonia_sans_n9", "helvetica_n3", "helvetica_n4", "helvetica_n7", "helvetica_n9",
    "helvetica_o3", "helvetica_o4", "helvetica_o7", "helvetica_o9", "hope_sans_i1", "hope_sans_i2",
    "hope_sans_i3", "hope_sans_i4", "hope_sans_i6", "hope_sans_i7", "hope_sans_n1", "hope_sans_n2",
    "hope_sans_n3", "hope_sans_n4", "hope_sans_n6", "hope_sans_n7", "humanist_521_i2", "humanist_521_i4",
    "humanist_521_i7", "humanist_521_n2", "humanist_521_n4", "humanist_521_n7", "humanist_521_n8",
    "humanist_521_n9", "itc_avant_garde_gothic_n2", "itc_avant_garde_gothic_n4", "itc_avant_garde_gothic_n5",
    "itc_avant_garde_gothic_n6", "itc_avant_garde_gothic_n7", "itc_avant_garde_gothic_o2",
    "itc_avant_garde_gothic_o4", "itc_avant_garde_gothic_o5", "itc_avant_garde_gothic_o6",
    "itc_avant_garde_gothic_o7", "itc_benguiat_i4", "itc_benguiat_i5", "itc_benguiat_i7", "itc_benguiat_n4",
    "itc_benguiat_n5", "itc_benguiat_n7", "itc_berkeley_old_style_i4", "itc_berkeley_old_style_i5",
    "itc_berkeley_old_style_i7", "itc_berkeley_old_style_i9", "itc_berkeley_old_style_n4",
    "itc_berkeley_old_style_n5", "itc_berkeley_old_style_n7", "itc_berkeley_old_style_n9",
    "itc_bodoni_seventytwo_i4", "itc_bodoni_seventytwo_i7", "itc_bodoni_seventytwo_n4",
    "itc_bodoni_seventytwo_n7", "itc_bodoni_twelve_i4", "itc_bodoni_twelve_i7", "itc_bodoni_twelve_n4",
    "itc_bodoni_twelve_n7", "itc_caslon_no_224_i4", "itc_caslon_no_224_i5", "itc_caslon_no_224_i7",
    "itc_caslon_no_224_i9", "itc_caslon_no_224_n4", "itc_caslon_no_224_n5", "itc_caslon_no_224_n7",
    "itc_caslon_no_224_n9", "itc_charter_i4", "itc_charter_i7", "itc_charter_i9", "itc_charter_n4",
    "itc_charter_n7", "itc_charter_n9", "itc_cheltenham_i3", "itc_cheltenham_i4", "itc_cheltenham_i7",
    "itc_cheltenham_i9", "itc_cheltenham_n3", "itc_cheltenham_n4", "itc_cheltenham_n7", "itc_cheltenham_n9",
    "itc_clearface_i4", "itc_clearface_i7", "itc_clearface_i8", "itc_clearface_i9", "itc_clearface_n4",
    "itc_clearface_n7", "itc_clearface_n8", "itc_clearface_n9", "itc_conduit_i2", "itc_conduit_i3",
    "itc_conduit_i4", "itc_conduit_i5", "itc_conduit_i7", "itc_conduit_i8", "itc_conduit_i9", "itc_conduit_n2",
    "itc_conduit_n3", "itc_conduit_n4", "itc_conduit_n5", "itc_conduit_n7", "itc_conduit_n8", "itc_conduit_n9",
    "itc_esprit_i4", "itc_esprit_i5", "itc_esprit_i7", "itc_esprit_i9", "itc_esprit_n4", "itc_esprit_n5",
    "itc_esprit_n7", "itc_esprit_n9", "itc_founders_caslon_i4", "itc_founders_caslon_n4",
    "itc_franklin_gothic_i4", "itc_franklin_gothic_i5", "itc_franklin_gothic_i7", "itc_franklin_gothic_i9",
    "itc_franklin_gothic_n4", "itc_franklin_gothic_n5", "itc_franklin_gothic_n7", "itc_franklin_gothic_n9",
    "itc_galliard_i4", "itc_galliard_i7", "itc_galliard_i8", "itc_galliard_i9", "itc_galliard_n4",
    "itc_galliard_n7", "itc_galliard_n8", "itc_galliard_n9", "itc_gamma_i4", "itc_gamma_i5", "itc_gamma_i7",
    "itc_gamma_i8", "itc_gamma_n4", "itc_gamma_n5", "itc_gamma_n7", "itc_gamma_n8", "itc_goudy_sans_i4",
    "itc_goudy_sans_i5", "itc_goudy_sans_i7", "itc_goudy_sans_i9", "itc_goudy_sans_n4", "itc_goudy_sans_n5",
    "itc_goudy_sans_n7", "itc_goudy_sans_n9", "itc_johnston_i3", "itc_johnston_i5", "itc_johnston_i7",
    "itc_johnston_n3", "itc_johnston_n5", "itc_johnston_n7", "itc_mendoza_roman_i4", "itc_mendoza_roman_i5",
    "itc_mendoza_roman_i7", "itc_mendoza_roman_n4", "itc_mendoza_roman_n5", "itc_mendoza_roman_n7",
    "itc_modern_no_216_i3", "itc_modern_no_216_i5", "itc_modern_no_216_i7", "itc_modern_no_216_i8",
    "itc_modern_no_216_n3", "itc_modern_no_216_n5", "itc_modern_no_216_n7", "itc_modern_no_216_n8",
    "itc_new_baskerville_i4", "itc_new_baskerville_i6", "itc_new_baskerville_i7", "itc_new_baskerville_i9",
    "itc_new_baskerville_n4", "itc_new_baskerville_n6", "itc_new_baskerville_n7", "itc_new_baskerville_n9",
    "itc_new_esprit_i4", "itc_new_esprit_i5", "itc_new_esprit_i7", "itc_new_esprit_i8", "itc_new_esprit_n4",
    "itc_new_esprit_n5", "itc_new_esprit_n7", "itc_new_esprit_n8", "itc_new_veljovic_i3", "itc_new_veljovic_i4",
    "itc_new_veljovic_i5", "itc_new_veljovic_i7", "itc_new_veljovic_i9", "itc_new_veljovic_n3",
    "itc_new_veljovic_n4", "itc_new_veljovic_n5", "itc_new_veljovic_n7", "itc_new_veljovic_n9", "itc_novarese_i4",
    "itc_novarese_i5", "itc_novarese_i7", "itc_novarese_n4", "itc_novarese_n5", "itc_novarese_n7",
    "itc_novarese_n9", "itc_officina_sans_i4", "itc_officina_sans_i5", "itc_officina_sans_i7",
    "itc_officina_sans_i8", "itc_officina_sans_i9", "itc_officina_sans_n4", "itc_officina_sans_n5",
    "itc_officina_sans_n7", "itc_officina_sans_n8", "itc_officina_sans_n9", "itc_officina_serif_i4",
    "itc_officina_serif_i5", "itc_officina_serif_i7", "itc_officina_serif_i8", "itc_officina_serif_i9",
    "itc_officina_serif_n4", "itc_officina_serif_n5", "itc_officina_serif_n7", "itc_officina_serif_n8",
    "itc_officina_serif_n9", "itc_stepp_i3", "itc_stepp_i5", "itc_stepp_i7", "itc_stepp_i9", "itc_stepp_n3",
    "itc_stepp_n4", "itc_stepp_n7", "itc_stepp_n9", "itc_stone_humanist_i5", "itc_stone_humanist_i6",
    "itc_stone_humanist_i7", "itc_stone_humanist_n5", "itc_stone_humanist_n6", "itc_stone_humanist_n7",
    "itc_stone_informal_i4", "itc_stone_informal_i6", "itc_stone_informal_i7", "itc_stone_informal_n4",
    "itc_stone_informal_n6", "itc_stone_informal_n7", "itc_stone_sans_ii_i3", "itc_stone_sans_ii_i4",
    "itc_stone_sans_ii_i5", "itc_stone_sans_ii_i6", "itc_stone_sans_ii_i7", "itc_stone_sans_ii_i8",
    "itc_stone_sans_ii_n3", "itc_stone_sans_ii_n4", "itc_stone_sans_ii_n5", "itc_stone_sans_ii_n6",
    "itc_stone_sans_ii_n7", "itc_stone_sans_ii_n8", "itc_stone_serif_i4", "itc_stone_serif_i6",
    "itc_stone_serif_i7", "itc_stone_serif_n4", "itc_stone_serif_n6", "itc_stone_serif_n7", "itc_tapioca_n5",
    "joanna_nova_i1", "joanna_nova_i2", "joanna_nova_i3", "joanna_nova_i4", "joanna_nova_i5", "joanna_nova_i6",
    "joanna_nova_i7", "joanna_nova_i8", "joanna_nova_i9", "joanna_nova_n1", "joanna_nova_n2", "joanna_nova_n3",
    "joanna_nova_n4", "joanna_nova_n5", "joanna_nova_n6", "joanna_nova_n7", "joanna_nova_n8", "joanna_nova_n9",
    "joanna_sans_nova_i1", "joanna_sans_nova_i2", "joanna_sans_nova_i3", "joanna_sans_nova_i4",
    "joanna_sans_nova_i5", "joanna_sans_nova_i7", "joanna_sans_nova_i8", "joanna_sans_nova_i9",
    "joanna_sans_nova_n1", "joanna_sans_nova_n2", "joanna_sans_nova_n3", "joanna_sans_nova_n4",
    "joanna_sans_nova_n5", "joanna_sans_nova_n7", "joanna_sans_nova_n8", "joanna_sans_nova_n9", "kairos_i1",
    "kairos_i2", "kairos_i3", "kairos_i4", "kairos_i5", "kairos_i7", "kairos_i8", "kairos_i9", "kairos_n1",
    "kairos_n2", "kairos_n3", "kairos_n4", "kairos_n5", "kairos_n7", "kairos_n8", "kairos_n9", "laurentian_i4",
    "laurentian_i6", "laurentian_n4", "laurentian_n6", "laurentian_n7", "libelle_n4", "linotype_didot_i4",
    "linotype_didot_i7", "linotype_didot_n4", "linotype_didot_n7", "linotype_gianotten_i3",
    "linotype_gianotten_i4", "linotype_gianotten_i5", "linotype_gianotten_i7", "linotype_gianotten_i8",
    "linotype_gianotten_n3", "linotype_gianotten_n4", "linotype_gianotten_n5", "linotype_gianotten_n7",
    "linotype_gianotten_n8", "linotype_gianotten_n9", "linotype_really_i5", "linotype_really_i6",
    "linotype_really_n5", "linotype_really_n6", "linotype_syntax_serif_i3", "linotype_syntax_serif_i4",
    "linotype_syntax_serif_i5", "linotype_syntax_serif_i7", "linotype_syntax_serif_i8",
    "linotype_syntax_serif_i9", "linotype_syntax_serif_n3", "linotype_syntax_serif_n4",
    "linotype_syntax_serif_n5", "linotype_syntax_serif_n7", "linotype_syntax_serif_n8",
    "linotype_syntax_serif_n9", "lobster_n4", "lucia_n4", "luthersche_fraktur_n4", "madera_i1", "madera_i2",
    "madera_i3", "madera_i4", "madera_i5", "madera_i6", "madera_i7", "madera_i8", "madera_n1", "madera_n2",
    "madera_n3", "madera_n4", "madera_n5", "madera_n6", "madera_n7", "madera_n8", "malabar_i4", "malabar_i7",
    "malabar_i9", "malabar_n4", "malabar_n7", "malabar_n9", "mariposa_sans_i4", "mariposa_sans_n4",
    "mariposa_sans_n5", "mariposa_sans_n7", "mariposa_sans_n9", "megrim_n4", "melior_i4", "melior_i7",
    "melior_n4", "melior_n7", "memphis_i3", "memphis_i5", "memphis_i7", "memphis_n3", "memphis_n5", "memphis_n7",
    "memphis_n8", "memphis_soft_rounded_n5", "memphis_soft_rounded_n7", "memphis_soft_rounded_n8",
    "mentor_sans_i3", "mentor_sans_i4", "mentor_sans_i7", "mentor_sans_i9", "mentor_sans_n3", "mentor_sans_n4",
    "mentor_sans_n7", "mentor_sans_n9", "metro_nova_i2", "metro_nova_i3", "metro_nova_i4", "metro_nova_i5",
    "metro_nova_i7", "metro_nova_i8", "metro_nova_i9", "metro_nova_n2", "metro_nova_n3", "metro_nova_n4",
    "metro_nova_n5", "metro_nova_n7", "metro_nova_n8", "metro_nova_n9", "modern_no_20_i4", "modern_no_20_n4",
    "monotype_baskerville_i4", "monotype_baskerville_i6", "monotype_baskerville_i7", "monotype_baskerville_n4",
    "monotype_baskerville_n6", "monotype_baskerville_n7", "monotype_bodoni_i4", "monotype_bodoni_i5",
    "monotype_bodoni_i7", "monotype_bodoni_i8", "monotype_bodoni_i9", "monotype_bodoni_n4", "monotype_bodoni_n5",
    "monotype_bodoni_n7", "monotype_bodoni_n8", "monotype_bodoni_n9", "monotype_century_old_style_i5",
    "monotype_century_old_style_i7", "monotype_century_old_style_n5", "monotype_century_old_style_n7",
    "monotype_goudy_i4", "monotype_goudy_i7", "monotype_goudy_modern_i4", "monotype_goudy_modern_i7",
    "monotype_goudy_modern_n4", "monotype_goudy_modern_n7", "monotype_goudy_n4", "monotype_goudy_n7",
    "monotype_goudy_n8", "monotype_italian_old_style_i4", "monotype_italian_old_style_i7",
    "monotype_italian_old_style_n4", "monotype_italian_old_style_n7", "monotype_new_clarendon_n5",
    "monotype_new_clarendon_n7", "monotype_news_gothic_i4", "monotype_news_gothic_i7", "monotype_news_gothic_n4",
    "monotype_news_gothic_n7", "monotype_sabon_i4", "monotype_sabon_i6", "monotype_sabon_n4", "monotype_sabon_n6",
    "mouse_memoirs_n4", "mundo_sans_i2", "mundo_sans_i3", "mundo_sans_i4", "mundo_sans_i5", "mundo_sans_i7",
    "mundo_sans_i8", "mundo_sans_i9", "mundo_sans_n2", "mundo_sans_n3", "mundo_sans_n4", "mundo_sans_n5",
    "mundo_sans_n7", "mundo_sans_n8", "mundo_sans_n9", "neo_sans_i3", "neo_sans_i4", "neo_sans_i5", "neo_sans_i7",
    "neo_sans_i8", "neo_sans_i9", "neo_sans_n3", "neo_sans_n4", "neo_sans_n5", "neo_sans_n7", "neo_sans_n8",
    "neo_sans_n9", "neue_aachen_i1", "neue_aachen_i2", "neue_aachen_i3", "neue_aachen_i4", "neue_aachen_i5",
    "neue_aachen_i6", "neue_aachen_i7", "neue_aachen_i8", "neue_aachen_i9", "neue_aachen_n1", "neue_aachen_n2",
    "neue_aachen_n3", "neue_aachen_n4", "neue_aachen_n5", "neue_aachen_n6", "neue_aachen_n7", "neue_aachen_n8",
    "neue_aachen_n9", "neue_frutiger_1450_i3", "neue_frutiger_1450_i4", "neue_frutiger_1450_i6",
    "neue_frutiger_1450_i7", "neue_frutiger_1450_n3", "neue_frutiger_1450_n4", "neue_frutiger_1450_n6",
    "neue_frutiger_1450_n7", "neue_haas_unica_i1", "neue_haas_unica_i2", "neue_haas_unica_i3",
    "neue_haas_unica_i4", "neue_haas_unica_i5", "neue_haas_unica_i6", "neue_haas_unica_i7", "neue_haas_unica_i8",
    "neue_haas_unica_i9", "neue_haas_unica_n1", "neue_haas_unica_n2", "neue_haas_unica_n3", "neue_haas_unica_n4",
    "neue_haas_unica_n5", "neue_haas_unica_n6", "neue_haas_unica_n7", "neue_haas_unica_n8", "neue_haas_unica_n9",
    "neue_plak_i2", "neue_plak_i3", "neue_plak_i4", "neue_plak_i6", "neue_plak_i7", "neue_plak_i9",
    "neue_plak_n2", "neue_plak_n3", "neue_plak_n4", "neue_plak_n6", "neue_plak_n7", "neue_plak_n9",
    "neue_swift_i3", "neue_swift_i4", "neue_swift_i5", "neue_swift_i6", "neue_swift_i7", "neue_swift_n3",
    "neue_swift_n4", "neue_swift_n5", "neue_swift_n6", "neue_swift_n7", "neuzeit_office_i4", "neuzeit_office_i7",
    "neuzeit_office_n4", "neuzeit_office_n7", "neuzeit_office_soft_rounded_n4", "neuzeit_office_soft_rounded_n7",
    "neuzeit_s_n4", "neuzeit_s_n9", "new_century_schoolbook_i4", "new_century_schoolbook_i7",
    "new_century_schoolbook_n4", "new_century_schoolbook_n7", "news_702_i4", "news_702_i7", "news_702_n4",
    "news_702_n7", "news_705_i4", "news_705_i7", "news_705_n4", "news_705_n7", "news_cycle_n4", "news_cycle_n7",
    "news_gothic_no_2_i2", "news_gothic_no_2_i3", "news_gothic_no_2_i4", "news_gothic_no_2_i5",
    "news_gothic_no_2_i7", "news_gothic_no_2_i8", "news_gothic_no_2_n2", "news_gothic_no_2_n3",
    "news_gothic_no_2_n4", "news_gothic_no_2_n5", "news_gothic_no_2_n7", "news_gothic_no_2_n8", "news_plantin_i4",
    "news_plantin_i7", "news_plantin_n4", "news_plantin_n7", "noticia_text_i4", "noticia_text_i7",
    "noticia_text_n4", "noticia_text_n7", "optima_nova_i3", "optima_nova_i4", "optima_nova_i5", "optima_nova_i6",
    "optima_nova_i7", "optima_nova_i8", "optima_nova_i9", "optima_nova_n3", "optima_nova_n4", "optima_nova_n5",
    "optima_nova_n6", "optima_nova_n7", "optima_nova_n8", "optima_nova_n9", "oxygen_n3", "oxygen_n4", "oxygen_n7",
    "parma_i4", "parma_n4", "parma_n7", "perpetua_i4", "perpetua_i7", "perpetua_n4", "perpetua_n7", "plantin_i3",
    "plantin_i4", "plantin_i6", "plantin_i7", "plantin_n3", "plantin_n4", "plantin_n6", "plantin_n7",
    "pmn_caecilia_i3", "pmn_caecilia_i4", "pmn_caecilia_i7", "pmn_caecilia_i9", "pmn_caecilia_n3",
    "pmn_caecilia_n4", "pmn_caecilia_n7", "pmn_caecilia_n9", "prata_n4", "pt_mono_n4", "pt_sans_narrow_n4",
    "pt_sans_narrow_n7", "quattrocento_n4", "quattrocento_n7", "quire_sans_i1", "quire_sans_i2", "quire_sans_i3",
    "quire_sans_i4", "quire_sans_i5", "quire_sans_i6", "quire_sans_i7", "quire_sans_i8", "quire_sans_i9",
    "quire_sans_n1", "quire_sans_n2", "quire_sans_n3", "quire_sans_n4", "quire_sans_n5", "quire_sans_n6",
    "quire_sans_n7", "quire_sans_n8", "quire_sans_n9", "really_no_2_i3", "really_no_2_i4", "really_no_2_i5",
    "really_no_2_i6", "really_no_2_i7", "really_no_2_i8", "really_no_2_i9", "really_no_2_n3", "really_no_2_n4",
    "really_no_2_n5", "really_no_2_n6", "really_no_2_n7", "really_no_2_n8", "really_no_2_n9", "righteous_n4",
    "rockwell_i4", "rockwell_i7", "rockwell_n4", "rockwell_n7", "rockwell_n8", "sabon_next_i4", "sabon_next_i6",
    "sabon_next_i7", "sabon_next_i8", "sabon_next_i9", "sabon_next_n4", "sabon_next_n6", "sabon_next_n7",
    "sabon_next_n8", "sabon_next_n9", "sackers_square_gothic_n4", "sagrantino_n4", "scene_i3", "scene_i4",
    "scene_i5", "scene_i7", "scene_i8", "scene_i9", "scene_n3", "scene_n4", "scene_n5", "scene_n7", "scene_n8",
    "scene_n9", "scherzo_i4", "scherzo_i6", "scherzo_i7", "scherzo_n4", "scherzo_n6", "scherzo_n7",
    "slabo_13px_n4", "slate_i2", "slate_i3", "slate_i4", "slate_i5", "slate_i7", "slate_i9", "slate_n2",
    "slate_n3", "slate_n4", "slate_n5", "slate_n7", "slate_n9", "soho_gothic_i2", "soho_gothic_i3",
    "soho_gothic_i4", "soho_gothic_i5", "soho_gothic_i7", "soho_gothic_i8", "soho_gothic_i9", "soho_gothic_n2",
    "soho_gothic_n3", "soho_gothic_n4", "soho_gothic_n5", "soho_gothic_n7", "soho_gothic_n8", "soho_gothic_n9",
    "soho_i1", "soho_i2", "soho_i3", "soho_i4", "soho_i5", "soho_i6", "soho_i7", "soho_i8", "soho_i9", "soho_n1",
    "soho_n2", "soho_n3", "soho_n4", "soho_n5", "soho_n6", "soho_n7", "soho_n8", "soho_n9",
    "stempel_schneidler_i3", "stempel_schneidler_i4", "stempel_schneidler_i5", "stempel_schneidler_i7",
    "stempel_schneidler_i8", "stempel_schneidler_n3", "stempel_schneidler_n4", "stempel_schneidler_n5",
    "stempel_schneidler_n7", "stempel_schneidler_n8", "swiss_721_i1", "swiss_721_i2", "swiss_721_i4",
    "swiss_721_i5", "swiss_721_i7", "swiss_721_i8", "swiss_721_i9", "swiss_721_n1", "swiss_721_n2",
    "swiss_721_n4", "swiss_721_n5", "swiss_721_n7", "swiss_721_n8", "swiss_721_n9", "swiss_721_rounded_n7",
    "swiss_721_rounded_n9", "tiemann_n3", "tiemann_n4", "tinos_i4", "tinos_i7", "tinos_n4", "tinos_n7",
    "trade_gothic_n3", "trade_gothic_n4", "trade_gothic_n7", "trade_gothic_next_i3", "trade_gothic_next_i4",
    "trade_gothic_next_i7", "trade_gothic_next_i8", "trade_gothic_next_n3", "trade_gothic_next_n4",
    "trade_gothic_next_n7", "trade_gothic_next_n8", "trade_gothic_o3", "trade_gothic_o4", "trade_gothic_o7",
    "twentieth_century_i3", "twentieth_century_i5", "twentieth_century_i7", "twentieth_century_i8",
    "twentieth_century_i9", "twentieth_century_n3", "twentieth_century_n4", "twentieth_century_n5",
    "twentieth_century_n6", "twentieth_century_n7", "twentieth_century_n8", "twentieth_century_n9",
    "univers_next_i1", "univers_next_i2", "univers_next_i3", "univers_next_i4", "univers_next_i5",
    "univers_next_i6", "univers_next_i7", "univers_next_i8", "univers_next_i9", "univers_next_n1",
    "univers_next_n2", "univers_next_n3", "univers_next_n4", "univers_next_n5", "univers_next_n6",
    "univers_next_n7", "univers_next_n8", "univers_next_n9", "univers_next_typewriter_i4",
    "univers_next_typewriter_i7", "univers_next_typewriter_n4", "univers_next_typewriter_n7", "unna_i4",
    "unna_i7", "unna_n4", "unna_n7", "vala_n4", "varela_n4", "varela_round_n4", "verdana_pro_i3",
    "verdana_pro_i4", "verdana_pro_i6", "verdana_pro_i7", "verdana_pro_i9", "verdana_pro_n3", "verdana_pro_n4",
    "verdana_pro_n6", "verdana_pro_n7", "verdana_pro_n9", "vidaloka_n4", "volkhov_i4", "volkhov_i7", "volkhov_n4",
    "volkhov_n7", "vollkorn_i4", "vollkorn_i5", "vollkorn_i6", "vollkorn_i7", "vollkorn_i8", "vollkorn_i9",
    "vollkorn_n4", "vollkorn_n5", "vollkorn_n6", "vollkorn_n7", "vollkorn_n8", "vollkorn_n9", "waza_n4",
    "wola_n4", "ysobel_i3", "ysobel_i4", "ysobel_i6", "ysobel_i7", "ysobel_n3", "ysobel_n4", "ysobel_n6",
    "ysobel_n7", "zurich_extended_n4", "zurich_extended_n7", "zurich_extended_n8", "zurich_extended_n9",
    "zurich_i3", "zurich_i4", "zurich_i7", "zurich_i8", "zurich_n3", "zurich_n4", "zurich_n7", "zurich_n8",
    "zurich_n9"
];

/// `DEPRECATED_FONT_HANDLES.has(value)`.
pub fn is_deprecated_font(handle: &str) -> bool {
    DEPRECATED_FONT_HANDLES.binary_search(&handle).is_ok()
}

/// The string value of a JSON literal node.
pub fn literal_str<'n>(node: &'n JsonNode<'_>) -> Option<&'n str> {
    match node {
        JsonNode::Literal { value: lq_json::Literal::String(s), .. } => Some(s),
        _ => None,
    }
}

/// `object.children.find((prop) => prop.key.value === key)`.
pub fn find_property<'n, 's>(object: &'n JsonNode<'s>, key: &str) -> Option<&'n JsonNode<'s>> {
    object.children().iter().find(|p| p.property_key() == Some(key))
}

/// `nodeAtPath(node, path)` with string keys, not tying the result to the path's lifetime.
pub fn node_at<'n, 's>(node: &'n JsonNode<'s>, path: &[String]) -> Option<&'n JsonNode<'s>> {
    let mut cur = node;
    for key in path {
        cur = match cur {
            JsonNode::Object { .. } => find_property(cur, key)?.property_value()?,
            JsonNode::Array { children, .. } => children.get(lq_json::value::array_index(key)? as usize)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// `getLocStart(node)`/`getLocEnd(node)` on `nodeAtPath(ast, path)!`: a missing node throws.
pub fn loc_at(ast: &JsonNode<'_>, path: &[String]) -> Js<lq_json::Loc> {
    node_at(ast, path)
        .map(JsonNode::loc)
        .ok_or_else(|| type_error("Cannot read properties of undefined (reading 'loc')"))
}

/// A byte offset `units` UTF-16 units past `start` in `source`. What JS gets from adding a
/// position in a decoded string to an offset in the source.
pub fn advance_utf16(source: &str, start: u32, units: u32) -> u32 {
    let mut left = units;
    let mut at = start as usize;
    let Some(rest) = source.get(at..) else { return start + units };
    for c in rest.chars() {
        if left == 0 {
            break;
        }
        left = left.saturating_sub(c.len_utf16() as u32);
        at += c.len_utf8();
    }
    at as u32 + left
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_table_is_sorted_and_complete() {
        assert_eq!(DEPRECATED_FONT_HANDLES.len(), 1341);
        assert!(DEPRECATED_FONT_HANDLES.windows(2).all(|w| w[0] < w[1]));
        assert!(is_deprecated_font("agmena_i3"));
        assert!(!is_deprecated_font("assistant_n4"));
    }

    #[test]
    fn deep_get_sees_prototypes() {
        let v = lq_json::parse_json(r#"{"a":{"b":"hello"},"c":[1]}"#).unwrap();
        let get = |p: &[&str]| deep_get(Some(&v), p, false);
        assert_eq!(get(&["a", "b", "length"]), DeepValue::Number(5.0));
        assert_eq!(get(&["a", "b", "0"]), DeepValue::Char("h".into()));
        assert_eq!(get(&["a", "b", "9"]), DeepValue::Undefined);
        assert_eq!(get(&["toString"]), DeepValue::Function);
        assert_eq!(get(&["nope"]), DeepValue::Undefined);
        assert_eq!(get(&["c", "length"]), DeepValue::Number(1.0));
        assert_eq!(deep_get(None, &["a"], false), DeepValue::Undefined);
    }

    #[test]
    fn scheduler_orders_by_time_then_fifo() {
        let log = std::cell::RefCell::new(Vec::new());
        let mut s = Sched::new();
        s.after(2, |_| log.borrow_mut().push("late"));
        s.after(1, |s| {
            log.borrow_mut().push("first");
            s.after(1, |_| log.borrow_mut().push("after-late"));
        });
        s.run();
        assert_eq!(*log.borrow(), ["first", "late", "after-late"]);
    }

    #[test]
    fn advance_counts_utf16_units() {
        assert_eq!(advance_utf16("abc", 0, 2), 2);
        assert_eq!(advance_utf16("é😀x", 0, 3), 6);
        assert_eq!(advance_utf16("ab", 0, 5), 5);
    }
}
