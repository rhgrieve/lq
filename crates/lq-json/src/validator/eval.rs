//! The `validate` function of `parser/jsonParser.js`, with each schema object's
//! keywords read once into a [`Compiled`] the first time it is visited.

use super::parser::{on_fresh_stack, Doc, Kind, NONE};
use super::schema::{schema_draft_from_id, Arena, EMPTY_OBJECT, FALSE_OBJECT, J};
use super::JsError;
use crate::js;
use crate::regex::JsRegex;
use std::borrow::Cow;
use std::sync::{Arc, OnceLock};

/// Recursion this deep only happens on a schema cycle that consumes no input, where
/// V8 throws `RangeError`. Ordinary documents stay far below it.
const MAX_DEPTH: u32 = 3000;
const STACK_HOP: u32 = 48;

type R = Result<(), JsError>;

/// A schema reference after `asSchema`; `Null` throws when validated against. Values
/// that are neither objects nor `null` have no keywords and act as `{}`.
#[derive(Clone, Copy, Debug)]
pub(super) enum Sch {
    Obj(u32),
    Null,
}

fn to_sch(j: &J) -> Sch {
    match j {
        J::Obj(o) => Sch::Obj(*o),
        J::Null => Sch::Null,
        _ => Sch::Obj(EMPTY_OBJECT),
    }
}

fn as_schema(j: &J) -> Sch {
    match j {
        J::Bool(true) => Sch::Obj(EMPTY_OBJECT),
        J::Bool(false) => Sch::Obj(FALSE_OBJECT),
        other => to_sch(other),
    }
}

/// `asSchema(value)` when that is truthy.
fn as_schema_truthy(j: Option<&J>) -> Option<Sch> {
    let j = j?;
    (matches!(j, J::Bool(_)) || j.truthy()).then(|| as_schema(j))
}

#[derive(Debug)]
struct TypeSpec {
    mask: u8,
    message: Box<str>,
}

const T_NULL: u8 = 1;
const T_BOOL: u8 = 2;
const T_ARRAY: u8 = 4;
const T_NUMBER: u8 = 8;
const T_STRING: u8 = 16;
const T_OBJECT: u8 = 32;
const T_INTEGER: u8 = 64;

fn type_bit(j: &J) -> u8 {
    match j.as_str().map(|s| &**s) {
        Some("null") => T_NULL,
        Some("boolean") => T_BOOL,
        Some("array") => T_ARRAY,
        Some("number") => T_NUMBER,
        Some("string") => T_STRING,
        Some("object") => T_OBJECT,
        Some("integer") => T_INTEGER,
        _ => 0,
    }
}

#[derive(Debug)]
struct Disc {
    key: Arc<str>,
    map: Vec<(Arc<str>, Vec<u32>)>,
}

#[derive(Debug)]
struct Alts {
    schs: Box<[Sch]>,
    object_disc: Result<Option<Disc>, JsError>,
    array_disc: Result<Option<Disc>, JsError>,
}

#[derive(Debug)]
struct Choice {
    values: Box<[J]>,
    message: Box<str>,
}

#[derive(Clone, Copy, Debug)]
enum PropSch {
    Bool(bool),
    Sch(Sch),
}

fn prop_sch(j: &J) -> PropSch {
    match j {
        J::Bool(b) => PropSch::Bool(*b),
        other => PropSch::Sch(to_sch(other)),
    }
}

#[derive(Debug)]
struct Required {
    key: Box<str>,
    message: Result<Box<str>, JsError>,
}

#[derive(Debug)]
enum Dep {
    Keys(Vec<(Box<str>, J)>),
    Sch(Option<Sch>),
}

#[derive(Debug, Default)]
struct ObjC {
    required: Vec<Required>,
    properties: Vec<(Arc<str>, PropSch)>,
    pattern_properties: Vec<(Pattern, PropSch)>,
    additional: Option<PropSch>,
    unevaluated: Option<PropSch>,
    max_properties: Option<f64>,
    min_properties: Option<f64>,
    dependent_required: Vec<(Arc<str>, Dep)>,
    dependent_schemas: Vec<(Arc<str>, Dep)>,
    dependencies: Vec<(Arc<str>, Dep)>,
    property_names: Option<Sch>,
}

#[derive(Clone, Debug)]
enum Prefix {
    Undefined,
    /// `null.length` throws.
    Null,
    Items(Vec<J>),
    /// A string: its characters are not schemas, so only the count matters.
    Count(usize),
    /// A value without a usable `length`.
    NoLength,
}

#[derive(Debug)]
struct ArrC {
    prefix_2020: Prefix,
    additional_2020: Option<J>,
    prefix_old: Prefix,
    additional_old: Option<J>,
    contains: Option<Sch>,
    min_contains: Option<f64>,
    max_contains: Option<f64>,
    unevaluated: Option<J>,
    min_items: Option<f64>,
    max_items: Option<f64>,
    unique_items: bool,
}

#[derive(Clone, Copy, Debug)]
enum FormatKind {
    Uri,
    UriReference,
    Regex(usize),
}

#[derive(Debug, Default)]
struct StrC {
    min_length: Option<f64>,
    max_length: Option<f64>,
    pattern: Option<(Pattern, Box<str>)>,
    format: Option<(FormatKind, Option<Box<str>>)>,
}

#[derive(Debug, Default)]
struct NumC {
    multiple_of: Option<f64>,
    exclusive_minimum: Option<f64>,
    exclusive_maximum: Option<f64>,
    minimum: Option<f64>,
    maximum: Option<f64>,
}

/// The keywords of one schema object, as `validate` reads them.
#[derive(Debug, Default)]
pub(crate) struct Compiled {
    types: Option<TypeSpec>,
    error_message: Option<Box<str>>,
    all_of: Option<Box<[Sch]>>,
    not: Option<Sch>,
    any_of: Option<Alts>,
    one_of: Option<Alts>,
    if_: Option<(Sch, Option<Sch>, Option<Sch>)>,
    enum_: Option<Choice>,
    const_: Option<Choice>,
    deprecation: Option<Box<str>>,
    object: Option<Box<ObjC>>,
    array: Option<Box<ArrC>>,
    string: Option<Box<StrC>>,
    number: Option<Box<NumC>>,
    uses_unevaluated: bool,
    /// `getSchemaDraft(schema)`, read for the root schema only.
    draft: Option<Result<Option<u8>, JsError>>,
}

/// `extendedRegExp`, with the shapes real schemas use matched without the engine.
#[derive(Debug)]
enum Pattern {
    Regex(JsRegex),
    /// `.*literal$`: `.` stops at line ends, but the match may start anywhere, so only
    /// the end of the text decides.
    Suffix(String),
    /// `^literal`.
    Prefix(String),
    /// `^[set]+$` over ASCII.
    SetPlus(Box<[bool; 128]>),
}

impl Pattern {
    fn is_match(&self, text: &str) -> bool {
        match self {
            Pattern::Regex(regex) => regex.is_match(text),
            Pattern::Suffix(suffix) => text.ends_with(suffix.as_str()),
            Pattern::Prefix(prefix) => text.starts_with(prefix.as_str()),
            Pattern::SetPlus(set) => {
                !text.is_empty() && text.bytes().all(|b| b < 128 && set[b as usize])
            }
        }
    }
}

fn simple_pattern(pattern: &str) -> Option<Pattern> {
    let literal =
        |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if let Some(suffix) = pattern.strip_prefix(".*").and_then(|p| p.strip_suffix('$')) {
        return literal(suffix).then(|| Pattern::Suffix(suffix.to_string()));
    }
    if let Some(body) = pattern
        .strip_prefix("^[")
        .and_then(|p| p.strip_suffix("]+$"))
    {
        let b = body.as_bytes();
        if b.is_empty()
            || b[0] == b'^'
            || !b
                .iter()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        {
            return None;
        }
        let mut set = Box::new([false; 128]);
        let mut i = 0;
        while i < b.len() {
            if i + 2 < b.len() && b[i + 1] == b'-' {
                if b[i] > b[i + 2] {
                    return None;
                }
                for c in b[i]..=b[i + 2] {
                    set[c as usize] = true;
                }
                i += 3;
            } else {
                set[b[i] as usize] = true;
                i += 1;
            }
        }
        return Some(Pattern::SetPlus(set));
    }
    if let Some(prefix) = pattern.strip_prefix('^') {
        let plain = prefix.strip_suffix(':').unwrap_or(prefix);
        return literal(plain).then(|| Pattern::Prefix(prefix.to_string()));
    }
    None
}

fn extended_regexp(pattern: &str) -> Option<Pattern> {
    if let Some(simple) = simple_pattern(pattern) {
        return Some(simple);
    }
    let (pattern, flags) = match pattern.strip_prefix("(?i)") {
        Some(rest) => (rest, "i"),
        None => (pattern, ""),
    };
    let with_u = format!("{flags}u");
    JsRegex::new(pattern, &with_u)
        .or_else(|_| JsRegex::new(pattern, flags))
        .ok()
        .map(Pattern::Regex)
}

fn stringify_list(arena: &Arena, values: &[J]) -> String {
    let mut out = String::new();
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        arena.stringify(v, &mut out, 0);
    }
    out
}

fn compile_alts(arena: &Arena, alts: &[J]) -> Alts {
    let schs: Box<[Sch]> = alts.iter().map(as_schema).collect();
    let object_disc = if alts.len() < 2 {
        Ok(None)
    } else {
        object_discriminator(arena, alts)
    };
    let array_disc = if alts.len() < 2 {
        Ok(None)
    } else {
        array_discriminator(arena, alts)
    };
    Alts {
        schs,
        object_disc,
        array_disc,
    }
}

/// `Map<key, Map<constValue, alternativeIndices>>`.
type ConstMap = Vec<(Arc<str>, Vec<(J, Vec<u32>)>)>;

/// A `constMap`, reduced to the first discriminator covering every alternative:
/// `findDiscriminator` stops there whether or not the value matches.
fn find_discriminator(const_map: ConstMap, alternatives: usize) -> Option<Disc> {
    for (key, values) in const_map {
        let mut covered: Vec<u32> = values.iter().flat_map(|(_, i)| i.iter().copied()).collect();
        covered.sort_unstable();
        covered.dedup();
        if covered.len() == alternatives {
            let map = values
                .into_iter()
                .filter_map(|(v, idx)| match v {
                    J::Str(s) => Some((s, idx)),
                    _ => None,
                })
                .collect();
            return Some(Disc { key, map });
        }
    }
    None
}

fn same_value_zero(a: &J, b: &J) -> bool {
    match (a, b) {
        (J::Null, J::Null) => true,
        (J::Bool(x), J::Bool(y)) => x == y,
        (J::Num(x), J::Num(y)) => x == y,
        (J::Str(x), J::Str(y)) => x == y,
        (J::Arr(x), J::Arr(y)) | (J::Obj(x), J::Obj(y)) => x == y,
        _ => false,
    }
}

fn add_consts(
    arena: &Arena,
    const_map: &mut ConstMap,
    entries: Vec<(Arc<str>, J)>,
    index: u32,
) -> R {
    for (key, schema) in entries {
        let sch = as_schema(&schema);
        let Sch::Obj(o) = sch else {
            return Err(JsError::type_error(
                "Cannot read properties of null (reading 'const')",
            ));
        };
        let Some(value) = arena.get(o, "const") else {
            continue;
        };
        let at = match const_map.iter().position(|(k, _)| *k == key) {
            Some(at) => at,
            None => {
                const_map.push((key, Vec::new()));
                const_map.len() - 1
            }
        };
        let values = &mut const_map[at].1;
        match values.iter_mut().find(|(v, _)| same_value_zero(v, value)) {
            Some((_, indices)) => indices.push(index),
            None => values.push((value.clone(), vec![index])),
        }
    }
    Ok(())
}

fn object_discriminator(arena: &Arena, alts: &[J]) -> Result<Option<Disc>, JsError> {
    let mut const_map = Vec::new();
    for (i, alt) in alts.iter().enumerate() {
        let Sch::Obj(o) = as_schema(alt) else {
            return Err(JsError::type_error(
                "Cannot read properties of null (reading 'properties')",
            ));
        };
        let Some(properties) = arena.get(o, "properties").filter(|p| p.truthy()) else {
            return Ok(None);
        };
        add_consts(
            arena,
            &mut const_map,
            arena.own_entries(properties),
            i as u32,
        )?;
    }
    Ok(find_discriminator(const_map, alts.len()))
}

fn array_discriminator(arena: &Arena, alts: &[J]) -> Result<Option<Disc>, JsError> {
    let mut const_map = Vec::new();
    for (i, alt) in alts.iter().enumerate() {
        let Sch::Obj(o) = as_schema(alt) else {
            return Err(JsError::type_error(
                "Cannot read properties of null (reading 'prefixItems')",
            ));
        };
        let prefix = arena.get(o, "prefixItems").filter(|p| p.truthy());
        let items = match prefix {
            Some(J::Arr(a)) => &arena.arrs[*a as usize],
            Some(_) => return Err(JsError::type_error("itemSchemas.map is not a function")),
            None => match arena.get(o, "items") {
                Some(J::Arr(a)) => &arena.arrs[*a as usize],
                _ => return Ok(None),
            },
        };
        let entries = items
            .iter()
            .enumerate()
            .map(|(k, v)| (Arc::from(k.to_string()), v.clone()))
            .collect();
        add_consts(arena, &mut const_map, entries, i as u32)?;
    }
    Ok(find_discriminator(const_map, alts.len()))
}

fn dep_keys(arena: &Arena, items: &[J]) -> Dep {
    Dep::Keys(
        items
            .iter()
            .map(|k| (arena.to_js_string(k, 0).into_boxed_str(), k.clone()))
            .collect(),
    )
}

fn compile_object(arena: &Arena, id: u32) -> Option<Box<ObjC>> {
    let get = |k: &str| arena.get(id, k);
    let mut c = ObjC::default();
    if let Some(required) = arena.arr(get("required")) {
        c.required = required
            .iter()
            .map(|name| Required {
                key: arena.to_js_string(name, 0).into_boxed_str(),
                message: arena
                    .t1("Missing property \"{0}\".", name)
                    .map(String::into_boxed_str),
            })
            .collect();
    }
    if let Some(properties) = get("properties").filter(|p| p.truthy()) {
        c.properties = arena
            .own_entries(properties)
            .into_iter()
            .map(|(k, v)| (k, prop_sch(&v)))
            .collect();
    }
    if let Some(patterns) = get("patternProperties").filter(|p| p.truthy()) {
        for (pattern, schema) in arena.own_entries(patterns) {
            if let Some(regex) = extended_regexp(&pattern) {
                c.pattern_properties.push((regex, prop_sch(&schema)));
            }
        }
    }
    c.additional = get("additionalProperties").map(prop_sch);
    c.unevaluated = get("unevaluatedProperties").map(prop_sch);
    c.max_properties = get("maxProperties").and_then(J::as_num);
    c.min_properties = get("minProperties").and_then(J::as_num);
    if let Some(deps) = get("dependentRequired").filter(|p| p.truthy()) {
        for (key, dep) in arena.own_entries(deps) {
            if let J::Arr(a) = dep {
                c.dependent_required
                    .push((key, dep_keys(arena, &arena.arrs[a as usize])));
            }
        }
    }
    if let Some(deps) = get("dependentSchemas").filter(|p| p.truthy()) {
        for (key, dep) in arena.own_entries(deps) {
            if matches!(dep, J::Obj(_)) {
                c.dependent_schemas
                    .push((key, Dep::Sch(as_schema_truthy(Some(&dep)))));
            }
        }
    }
    if let Some(deps) = get("dependencies").filter(|p| p.truthy()) {
        for (key, dep) in arena.own_entries(deps) {
            let dep = match dep {
                J::Arr(a) => dep_keys(arena, &arena.arrs[a as usize]),
                other => Dep::Sch(as_schema_truthy(Some(&other))),
            };
            c.dependencies.push((key, dep));
        }
    }
    c.property_names = as_schema_truthy(get("propertyNames"));
    let empty = c.required.is_empty()
        && c.properties.is_empty()
        && c.pattern_properties.is_empty()
        && c.additional.is_none()
        && c.unevaluated.is_none()
        && c.max_properties.is_none()
        && c.min_properties.is_none()
        && c.dependent_required.is_empty()
        && c.dependent_schemas.is_empty()
        && c.dependencies.is_empty()
        && c.property_names.is_none();
    (!empty).then(|| Box::new(c))
}

fn to_prefix(value: Option<&J>) -> Prefix {
    match value {
        None => Prefix::Undefined,
        Some(J::Null) => Prefix::Null,
        Some(J::Str(s)) => Prefix::Count(js::utf16_len(s)),
        Some(_) => Prefix::NoLength,
    }
}

fn compile_array(arena: &Arena, id: u32) -> Option<Box<ArrC>> {
    let get = |k: &str| arena.get(id, k);
    let items = get("items");
    let items_array = arena.arr(items);
    let prefix_items = match get("prefixItems") {
        Some(J::Arr(a)) => Prefix::Items(arena.arrs[*a as usize].clone()),
        other => to_prefix(other),
    };
    let (prefix_old, additional_old) = match items_array {
        Some(array) => (
            Prefix::Items(array.to_vec()),
            get("additionalItems").cloned(),
        ),
        None => (Prefix::Undefined, items.cloned()),
    };
    let c = ArrC {
        prefix_2020: prefix_items,
        additional_2020: if items_array.is_some() {
            None
        } else {
            items.cloned()
        },
        prefix_old,
        additional_old,
        contains: as_schema_truthy(get("contains")),
        min_contains: get("minContains").and_then(J::as_num),
        max_contains: get("maxContains").and_then(J::as_num),
        unevaluated: get("unevaluatedItems").cloned(),
        min_items: get("minItems").and_then(J::as_num),
        max_items: get("maxItems").and_then(J::as_num),
        unique_items: matches!(get("uniqueItems"), Some(J::Bool(true))),
    };
    let empty = matches!(c.prefix_2020, Prefix::Undefined)
        && c.additional_2020.is_none()
        && matches!(c.prefix_old, Prefix::Undefined)
        && c.additional_old.is_none()
        && c.contains.is_none()
        && c.unevaluated.is_none()
        && c.min_items.is_none()
        && c.max_items.is_none()
        && !c.unique_items;
    (!empty).then(|| Box::new(c))
}

fn truthy_text(arena: &Arena, value: Option<&J>) -> Option<Box<str>> {
    value
        .filter(|v| v.truthy())
        .map(|v| arena.to_js_string(v, 0).into_boxed_str())
}

fn compile_string(arena: &Arena, id: u32) -> Option<Box<StrC>> {
    let get = |k: &str| arena.get(id, k);
    let mut c = StrC {
        min_length: get("minLength").and_then(J::as_num),
        max_length: get("maxLength").and_then(J::as_num),
        ..StrC::default()
    };
    let specific = truthy_text(arena, get("patternErrorMessage"))
        .or_else(|| truthy_text(arena, get("errorMessage")));
    if let Some(J::Str(pattern)) = get("pattern") {
        if let Some(regex) = extended_regexp(pattern) {
            let message = specific.clone().unwrap_or_else(|| {
                format!("String does not match the pattern of \"{pattern}\".").into_boxed_str()
            });
            c.pattern = Some((regex, message));
        }
    }
    if let Some(J::Str(format)) = get("format") {
        let kind = match &**format {
            "uri" => Some(FormatKind::Uri),
            "uri-reference" => Some(FormatKind::UriReference),
            "color-hex" => Some(FormatKind::Regex(0)),
            "date-time" => Some(FormatKind::Regex(1)),
            "date" => Some(FormatKind::Regex(2)),
            "time" => Some(FormatKind::Regex(3)),
            "email" => Some(FormatKind::Regex(4)),
            "hostname" => Some(FormatKind::Regex(5)),
            "ipv4" => Some(FormatKind::Regex(6)),
            "ipv6" => Some(FormatKind::Regex(7)),
            _ => None,
        };
        c.format = kind.map(|k| (k, specific));
    }
    let empty = c.min_length.is_none()
        && c.max_length.is_none()
        && c.pattern.is_none()
        && c.format.is_none();
    (!empty).then(|| Box::new(c))
}

fn compile_number(arena: &Arena, id: u32) -> Option<Box<NumC>> {
    let get = |k: &str| arena.get(id, k);
    let exclusive_limit = |limit: Option<&J>, exclusive: Option<&J>| match exclusive {
        Some(J::Num(n)) => Some(*n),
        Some(J::Bool(true)) => limit.and_then(J::as_num),
        _ => None,
    };
    let limit = |limit: Option<&J>, exclusive: Option<&J>| match exclusive {
        Some(J::Bool(true)) => None,
        _ => limit.and_then(J::as_num),
    };
    let c = NumC {
        multiple_of: get("multipleOf").and_then(J::as_num),
        exclusive_minimum: exclusive_limit(get("minimum"), get("exclusiveMinimum")),
        exclusive_maximum: exclusive_limit(get("maximum"), get("exclusiveMaximum")),
        minimum: limit(get("minimum"), get("exclusiveMinimum")),
        maximum: limit(get("maximum"), get("exclusiveMaximum")),
    };
    let empty = c.multiple_of.is_none()
        && c.exclusive_minimum.is_none()
        && c.exclusive_maximum.is_none()
        && c.minimum.is_none()
        && c.maximum.is_none();
    (!empty).then(|| Box::new(c))
}

fn compile(arena: &Arena, id: u32) -> Compiled {
    let get = |k: &str| arena.get(id, k);
    let error_message = truthy_text(arena, get("errorMessage"));
    let or_error = |fallback: String| -> Box<str> {
        error_message
            .clone()
            .unwrap_or_else(|| fallback.into_boxed_str())
    };
    let mut c = Compiled::default();
    match get("type") {
        Some(J::Arr(a)) => {
            let types = &arena.arrs[*a as usize];
            let mask = types.iter().fold(0, |m, t| m | type_bit(t));
            let message = format!(
                "Incorrect type. Expected one of {}.",
                arena.join(types, ", ", 0)
            );
            c.types = Some(TypeSpec {
                mask,
                message: or_error(message),
            });
        }
        Some(t) if t.truthy() => {
            let message = arena
                .t1("Incorrect type. Expected \"{0}\".", t)
                .unwrap_or_else(|_| String::new());
            c.types = Some(TypeSpec {
                mask: type_bit(t),
                message: or_error(message),
            });
        }
        _ => {}
    }
    c.all_of = arena
        .arr(get("allOf"))
        .map(|a| a.iter().map(as_schema).collect());
    c.not = as_schema_truthy(get("not"));
    c.any_of = arena.arr(get("anyOf")).map(|a| compile_alts(arena, a));
    c.one_of = arena.arr(get("oneOf")).map(|a| compile_alts(arena, a));
    if let Some(if_) = as_schema_truthy(get("if")) {
        c.if_ = Some((
            if_,
            as_schema_truthy(get("then")),
            as_schema_truthy(get("else")),
        ));
    }
    if let Some(values) = arena.arr(get("enum")) {
        let message = format!(
            "Value is not accepted. Valid values: {}.",
            stringify_list(arena, values)
        );
        c.enum_ = Some(Choice {
            values: values.to_vec().into_boxed_slice(),
            message: or_error(message),
        });
    }
    if let Some(value) = get("const") {
        let mut text = String::new();
        arena.stringify(value, &mut text, 0);
        c.const_ = Some(Choice {
            values: vec![value.clone()].into_boxed_slice(),
            message: or_error(format!("Value must be {text}.")),
        });
    }
    let deprecation = truthy_text(arena, get("deprecationMessage"));
    c.deprecation = match deprecation {
        Some(m) => Some(m),
        None if get("deprecated").is_some_and(J::truthy) => Some("Value is deprecated".into()),
        None => None,
    };
    c.object = compile_object(arena, id);
    c.array = compile_array(arena, id);
    c.string = compile_string(arena, id);
    c.number = compile_number(arena, id);
    c.uses_unevaluated =
        get("unevaluatedProperties").is_some() || get("unevaluatedItems").is_some();
    c.draft = get("$schema")
        .filter(|v| v.truthy())
        .map(schema_draft_from_id);
    c.error_message = error_message;
    c
}

impl Arena {
    fn compiled(&self, id: u32) -> &Compiled {
        self.objs[id as usize]
            .compiled
            .get_or_init(|| Box::new(compile(self, id)))
    }
}

fn format_regex(i: usize) -> &'static (JsRegex, &'static str) {
    static FORMATS: [OnceLock<(JsRegex, &'static str)>; 8] = [const { OnceLock::new() }; 8];
    FORMATS[i].get_or_init(|| {
        let (pattern, flags, message) = FORMAT_SOURCES[i];
        (JsRegex::new(pattern, flags).expect("format regex"), message)
    })
}

const FORMAT_SOURCES: [(&str, &str, &str); 8] = [
    (
        r"^#([0-9A-Fa-f]{3,4}|([0-9A-Fa-f]{2}){3,4})$",
        "",
        "Invalid color format. Use #RGB, #RGBA, #RRGGBB or #RRGGBBAA.",
    ),
    (
        r"^(\d{4})-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):([0-5][0-9]):([0-5][0-9]|60)(\.[0-9]+)?(Z|(\+|-)([01][0-9]|2[0-3]):([0-5][0-9]))$",
        "i",
        "String is not a RFC3339 date-time.",
    ),
    (
        r"^(\d{4})-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])$",
        "i",
        "String is not a RFC3339 date.",
    ),
    (
        r"^([01][0-9]|2[0-3]):([0-5][0-9]):([0-5][0-9]|60)(\.[0-9]+)?(Z|(\+|-)([01][0-9]|2[0-3]):([0-5][0-9]))$",
        "i",
        "String is not a RFC3339 time.",
    ),
    (
        r#"^(([^<>()\[\]\\.,;:\s@"]+(\.[^<>()\[\]\\.,;:\s@"]+)*)|(".+"))@((\[[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}])|(([a-zA-Z0-9-]+\.)+[a-zA-Z]{2,}))$"#,
        "",
        "String is not an e-mail address.",
    ),
    (
        r"^(?=.{1,253}\.?$)[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[-0-9a-z]{0,61}[0-9a-z])?)*\.?$",
        "i",
        "String is not a hostname.",
    ),
    (
        r"^(?:(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\.){3}(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)$",
        "",
        "String is not an IPv4 address.",
    ),
    (
        r"^((([0-9a-f]{1,4}:){7}([0-9a-f]{1,4}|:))|(([0-9a-f]{1,4}:){6}(:[0-9a-f]{1,4}|((25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3})|:))|(([0-9a-f]{1,4}:){5}(((:[0-9a-f]{1,4}){1,2})|:((25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3})|:))|(([0-9a-f]{1,4}:){4}(((:[0-9a-f]{1,4}){1,3})|((:[0-9a-f]{1,4})?:((25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}))|:))|(([0-9a-f]{1,4}:){3}(((:[0-9a-f]{1,4}){1,4})|((:[0-9a-f]{1,4}){0,2}:((25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}))|:))|(([0-9a-f]{1,4}:){2}(((:[0-9a-f]{1,4}){1,5})|((:[0-9a-f]{1,4}){0,3}:((25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}))|:))|(([0-9a-f]{1,4}:){1}(((:[0-9a-f]{1,4}){1,6})|((:[0-9a-f]{1,4}){0,4}:((25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}))|:))|(:(((:[0-9a-f]{1,4}){1,7})|((:[0-9a-f]{1,4}){0,5}:((25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}))|:)))$",
        "i",
        "String is not an IPv6 address.",
    ),
];

/// A problem's message, formatted only if the problem survives alternative matching.
#[derive(Debug)]
pub(super) enum Msg<'a> {
    Str(&'a str),
    Owned(String),
    NotAllowed(&'a str),
    Enum(Cow<'a, [J]>),
}

impl Msg<'_> {
    pub fn render(&self, arena: &Arena) -> String {
        match self {
            Msg::Str(s) => (*s).to_string(),
            Msg::Owned(s) => s.clone(),
            Msg::NotAllowed(name) => format!("Property {name} is not allowed."),
            Msg::Enum(values) => {
                format!(
                    "Value is not accepted. Valid values: {}.",
                    stringify_list(arena, values)
                )
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct Problem<'a> {
    pub offset: u32,
    pub length: u32,
    pub message: Msg<'a>,
    pub warning: bool,
    enum_mismatch: bool,
}

/// `processedProperties`: small and rare, as only `unevaluated*` reads it.
#[derive(Debug, Default)]
struct Processed<'a>(Vec<Cow<'a, str>>);

impl<'a> Processed<'a> {
    fn insert(&mut self, name: Cow<'a, str>) {
        if !self.contains(&name) {
            self.0.push(name);
        }
    }
    fn contains(&self, name: &str) -> bool {
        self.0.iter().any(|n| n == name)
    }
}

/// `ValidationResult`. `processed` is only kept below schemas that read it.
#[derive(Debug, Default)]
pub(super) struct Vr<'a> {
    pub problems: Vec<Problem<'a>>,
    properties_matches: u32,
    properties_value_matches: u32,
    primary_value_matches: u32,
    enum_value_match: bool,
    enum_values: Option<Cow<'a, [J]>>,
    processed: Processed<'a>,
    /// Only whether there are problems matters here (inside `if`, `not`, `contains`).
    /// They are then counted, and failing an `if` allocates nothing.
    quiet: bool,
    quiet_problems: u32,
}

impl<'a> Vr<'a> {
    fn quiet() -> Vr<'a> {
        Vr {
            quiet: true,
            ..Vr::default()
        }
    }

    /// A result for a nested validation that reports into this one.
    fn nested(&self) -> Vr<'a> {
        Vr {
            quiet: self.quiet,
            ..Vr::default()
        }
    }

    fn has_problems(&self) -> bool {
        self.quiet_problems != 0 || !self.problems.is_empty()
    }

    fn add(&mut self, problem: Problem<'a>) {
        if self.quiet {
            self.quiet_problems += 1;
        } else {
            self.problems.push(problem);
        }
    }

    fn merge_processed(&mut self, other: &mut Vr<'a>) {
        for name in other.processed.0.drain(..) {
            self.processed.insert(name);
        }
    }

    fn merge(&mut self, mut other: Vr<'a>) {
        self.problems.append(&mut other.problems);
        self.quiet_problems += other.quiet_problems;
        self.properties_matches += other.properties_matches;
        self.properties_value_matches += other.properties_value_matches;
        self.merge_processed(&mut other);
    }

    fn merge_enum_values(&mut self, other: &Vr<'a>) {
        if !self.enum_value_match && !other.enum_value_match {
            if let (Some(a), Some(b)) = (&mut self.enum_values, &other.enum_values) {
                a.to_mut().extend_from_slice(b);
            }
        }
    }

    fn update_enum_mismatch_problem_messages(&mut self) {
        if !self.enum_value_match {
            if let Some(values) = &self.enum_values {
                for p in &mut self.problems {
                    if p.enum_mismatch {
                        p.message = Msg::Enum(values.clone());
                    }
                }
            }
        }
    }

    fn merge_property_match(&mut self, mut other: Vr<'a>) {
        let has_problems = other.has_problems();
        self.problems.append(&mut other.problems);
        self.quiet_problems += other.quiet_problems;
        self.properties_matches += 1;
        if other.enum_value_match || (!has_problems && other.properties_matches != 0) {
            self.properties_value_matches += 1;
        }
        if other.enum_value_match && other.enum_values.as_ref().is_some_and(|v| v.len() == 1) {
            self.primary_value_matches += 1;
        }
    }

    fn compare(&self, other: &Vr<'a>) -> i64 {
        let has_problems = self.has_problems();
        if has_problems != other.has_problems() {
            return if has_problems { -1 } else { 1 };
        }
        if self.enum_value_match != other.enum_value_match {
            return if other.enum_value_match { -1 } else { 1 };
        }
        if self.primary_value_matches != other.primary_value_matches {
            return i64::from(self.primary_value_matches) - i64::from(other.primary_value_matches);
        }
        if self.properties_value_matches != other.properties_value_matches {
            return i64::from(self.properties_value_matches)
                - i64::from(other.properties_value_matches);
        }
        i64::from(self.properties_matches) - i64::from(other.properties_matches)
    }
}

/// `unprocessedProperties`, over a [`Doc::uniq`] slice.
enum Unprocessed {
    Small(u64),
    Large(Vec<bool>),
}

impl Unprocessed {
    fn new(count: usize) -> Unprocessed {
        if count <= 64 {
            Unprocessed::Small(if count == 64 {
                u64::MAX
            } else {
                (1u64 << count) - 1
            })
        } else {
            Unprocessed::Large(vec![true; count])
        }
    }
    fn has(&self, i: usize) -> bool {
        match self {
            Unprocessed::Small(bits) => bits & (1 << i) != 0,
            Unprocessed::Large(v) => v[i],
        }
    }
    fn delete(&mut self, i: usize) {
        match self {
            Unprocessed::Small(bits) => *bits &= !(1 << i),
            Unprocessed::Large(v) => v[i] = false,
        }
    }
}

pub(super) struct Eval<'a, 's> {
    pub doc: &'a Doc<'s>,
    pub arena: &'a Arena,
    pub draft_2020: bool,
}

/// `getSchemaDraft(schema) >= SchemaDraft.v2020_12`.
pub(super) fn root_draft_2020(arena: &Arena, root: &J) -> Result<bool, JsError> {
    let J::Obj(o) = root else { return Ok(true) };
    match &arena.compiled(*o).draft {
        None => Ok(true),
        Some(Err(e)) => Err(e.clone()),
        Some(Ok(draft)) => Ok(draft.unwrap_or(20) >= 20),
    }
}

/// `schemaAllowsComments`/`schemaAllowsTrailingCommas`, for the exceptions they can
/// throw: with comments and trailing commas ignored, their answer has no effect.
pub(super) fn allows(
    arena: &Arena,
    schema: &J,
    keys: &[&str],
    depth: u32,
) -> Result<Option<bool>, JsError> {
    let J::Obj(o) = schema else { return Ok(None) };
    if depth > 4000 {
        return Err(JsError::range_error());
    }
    for key in keys {
        if let Some(J::Bool(b)) = arena.get(*o, key) {
            return Ok(Some(*b));
        }
    }
    if let Some(all_of) = arena.get(*o, "allOf").filter(|a| a.truthy()) {
        match all_of {
            J::Arr(a) => {
                for sub in &arena.arrs[*a as usize] {
                    if let Some(allow) = allows(arena, sub, keys, depth + 1)? {
                        return Ok(Some(allow));
                    }
                }
            }
            J::Str(_) => {}
            _ => return Err(JsError::type_error("schemaRef.allOf is not iterable")),
        }
    }
    Ok(None)
}

impl<'a> Eval<'a, '_> {
    pub fn validate(&self, n: u32, sch: Sch, vr: &mut Vr<'a>, depth: u32, track: bool) -> R {
        let Sch::Obj(id) = sch else {
            return Err(JsError::type_error(
                "Cannot read properties of null (reading 'type')",
            ));
        };
        if depth >= MAX_DEPTH {
            return Err(JsError::range_error());
        }
        if depth % STACK_HOP == STACK_HOP - 1 {
            return on_fresh_stack(|| self.validate_compiled(n, id, vr, depth, track));
        }
        self.validate_compiled(n, id, vr, depth, track)
    }

    fn validate_compiled(&self, n: u32, id: u32, vr: &mut Vr<'a>, depth: u32, track: bool) -> R {
        let c: &'a Compiled = self.arena.compiled(id);
        let track = track || c.uses_unevaluated;
        self.validate_node(n, c, vr, depth, track)?;
        match &self.doc.nodes[n as usize].kind {
            Kind::Object { .. } => {
                if let Some(o) = &c.object {
                    self.validate_object(n, c, o, vr, depth, track)?;
                }
            }
            Kind::Array { .. } => {
                if let Some(a) = &c.array {
                    self.validate_array(n, c, a, vr, depth, track)?;
                }
            }
            Kind::String(value) => {
                if let Some(s) = &c.string {
                    self.validate_string(n, value, s, vr);
                }
            }
            Kind::Number { value, .. } => {
                if let Some(num) = &c.number {
                    self.validate_number(n, *value, num, vr);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn push(&self, n: u32, vr: &mut Vr<'a>, message: Msg<'a>) {
        let node = &self.doc.nodes[n as usize];
        vr.add(Problem {
            offset: node.offset,
            length: node.length,
            message,
            warning: false,
            enum_mismatch: false,
        });
    }

    fn validate_node(
        &self,
        n: u32,
        c: &'a Compiled,
        vr: &mut Vr<'a>,
        depth: u32,
        track: bool,
    ) -> R {
        let node = &self.doc.nodes[n as usize];
        if let Some(t) = &c.types {
            let kind_bits = match node.kind {
                Kind::Null => T_NULL,
                Kind::Bool => T_BOOL,
                Kind::Array { .. } => T_ARRAY,
                Kind::Number { is_integer, .. } => {
                    if is_integer {
                        T_NUMBER | T_INTEGER
                    } else {
                        T_NUMBER
                    }
                }
                Kind::String(_) => T_STRING,
                Kind::Object { .. } => T_OBJECT,
                Kind::Property { .. } => 0,
            };
            if t.mask & kind_bits == 0 {
                self.push(n, vr, Msg::Str(&t.message));
            }
        }
        if let Some(all_of) = &c.all_of {
            for &sub in &**all_of {
                let mut s = vr.nested();
                self.validate(n, sub, &mut s, depth + 1, track)?;
                vr.merge(s);
            }
        }
        if let Some(not) = c.not {
            let mut s = Vr::quiet();
            self.validate(n, not, &mut s, depth + 1, track)?;
            if !s.has_problems() {
                let message = match &c.error_message {
                    Some(m) => Msg::Str(m),
                    None => Msg::Str("Matches a schema that is not allowed."),
                };
                self.push(n, vr, message);
            }
        }
        if let Some(alts) = &c.any_of {
            self.test_alternatives(n, alts, false, vr, depth, track)?;
        }
        if let Some(alts) = &c.one_of {
            self.test_alternatives(n, alts, true, vr, depth, track)?;
        }
        if let Some((if_, then, else_)) = c.if_ {
            let mut s = Vr::quiet();
            self.validate(n, if_, &mut s, depth + 1, track)?;
            vr.merge_processed(&mut s);
            let branch = if s.has_problems() { else_ } else { then };
            if let Some(branch) = branch {
                let mut b = vr.nested();
                self.validate(n, branch, &mut b, depth + 1, track)?;
                vr.merge(b);
            }
        }
        if let Some(e) = &c.enum_ {
            let matched = e.values.iter().any(|v| self.equals_schema(n, v));
            vr.enum_values = Some(Cow::Borrowed(&e.values));
            vr.enum_value_match = matched;
            if !matched {
                let node = &self.doc.nodes[n as usize];
                vr.add(Problem {
                    offset: node.offset,
                    length: node.length,
                    message: Msg::Str(&e.message),
                    warning: false,
                    enum_mismatch: true,
                });
            }
        }
        if let Some(k) = &c.const_ {
            let matched = self.equals_schema(n, &k.values[0]);
            if !matched {
                let node = &self.doc.nodes[n as usize];
                vr.add(Problem {
                    offset: node.offset,
                    length: node.length,
                    message: Msg::Str(&k.message),
                    warning: false,
                    enum_mismatch: true,
                });
            }
            vr.enum_value_match = matched;
            vr.enum_values = Some(Cow::Borrowed(&k.values));
        }
        if let Some(message) = &c.deprecation {
            let node = &self.doc.nodes[n as usize];
            let target = if node.parent != NONE
                && matches!(
                    self.doc.nodes[node.parent as usize].kind,
                    Kind::Property { .. }
                ) {
                &self.doc.nodes[node.parent as usize]
            } else {
                node
            };
            vr.add(Problem {
                offset: target.offset,
                length: target.length,
                message: Msg::Str(message),
                warning: true,
                enum_mismatch: false,
            });
        }
        Ok(())
    }

    fn discriminator(&self, n: u32, alts: &'a Alts) -> Result<Option<&'a [u32]>, JsError> {
        if alts.schs.len() < 2 {
            return Ok(None);
        }
        match &self.doc.nodes[n as usize].kind {
            Kind::Object { props: (a, b), .. } if b > a => {
                let Some(disc) = alts.object_disc.as_ref().map_err(Clone::clone)? else {
                    return Ok(None);
                };
                let first = self.doc.props(n).iter().copied().find(|&p| {
                    let (k, _) = self.doc.property(p);
                    self.doc.str_value(k) == &*disc.key
                });
                let value = first.and_then(|p| {
                    let (_, v) = self.doc.property(p);
                    match v {
                        NONE => None,
                        v => match &self.doc.nodes[v as usize].kind {
                            Kind::String(s) => Some(s.as_ref()),
                            _ => None,
                        },
                    }
                });
                Ok(value.and_then(|v| {
                    disc.map
                        .iter()
                        .find(|(k, _)| &**k == v)
                        .map(|(_, i)| i.as_slice())
                }))
            }
            Kind::Array { kids: (a, b) } if b > a => {
                let Some(disc) = alts.array_disc.as_ref().map_err(Clone::clone)? else {
                    return Ok(None);
                };
                let items = self.doc.items(n);
                let item = disc
                    .key
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| items.get(i).copied());
                let value = item.and_then(|i| match &self.doc.nodes[i as usize].kind {
                    Kind::String(s) => Some(s.as_ref()),
                    _ => None,
                });
                Ok(value.and_then(|v| {
                    disc.map
                        .iter()
                        .find(|(k, _)| &**k == v)
                        .map(|(_, i)| i.as_slice())
                }))
            }
            _ => Ok(None),
        }
    }

    fn test_alternatives(
        &self,
        n: u32,
        alts: &'a Alts,
        max_one_match: bool,
        vr: &mut Vr<'a>,
        depth: u32,
        track: bool,
    ) -> R {
        let chosen = self.discriminator(n, alts)?;
        let count = chosen.map_or(alts.schs.len(), <[u32]>::len);
        let mut matches = 0;
        let mut best: Option<Vr<'a>> = None;
        for i in 0..count {
            let sub = match chosen {
                Some(indices) => alts.schs[indices[i] as usize],
                None => alts.schs[i],
            };
            let mut s = vr.nested();
            self.validate(n, sub, &mut s, depth + 1, track)?;
            if !s.has_problems() {
                matches += 1;
            }
            match &mut best {
                None => best = Some(s),
                Some(b) => {
                    if !max_one_match && !s.has_problems() && !b.has_problems() {
                        b.properties_matches += s.properties_matches;
                        b.properties_value_matches += s.properties_value_matches;
                        b.merge_processed(&mut s);
                    } else {
                        let compare = s.compare(b);
                        if compare > 0 {
                            *b = s;
                        } else if compare == 0 {
                            b.merge_enum_values(&s);
                        }
                    }
                }
            }
        }
        if matches > 1 && max_one_match {
            // One code unit. A property name in `propertyNames` may be an unquoted word,
            // and then the unit can be a whole character or half of an astral one.
            let offset = self.doc.nodes[n as usize].offset;
            let length = match self.doc.text[offset as usize..].chars().next() {
                Some(c) if c.len_utf8() == 4 => 2,
                Some(c) => c.len_utf8() as u32,
                None => 1,
            };
            vr.add(Problem {
                offset,
                length,
                message: Msg::Str("Matches multiple schemas when only one must validate."),
                warning: false,
                enum_mismatch: false,
            });
        }
        if let Some(mut b) = best {
            b.update_enum_mismatch_problem_messages();
            vr.merge(b);
        }
        Ok(())
    }

    fn validate_number(&self, n: u32, val: f64, c: &NumC, vr: &mut Vr<'a>) {
        if let Some(multiple_of) = c.multiple_of {
            let mut remainder = -1.0;
            if multiple_of.is_finite() && multiple_of.trunc() == multiple_of {
                remainder = val % multiple_of;
            } else if let (Some(m), Some(v)) =
                (normalize_floats(multiple_of), normalize_floats(val))
            {
                let (mut m_value, mut v_value) = (m.0, v.0);
                let multiplier = 10f64.powf(f64::from((v.1 - m.1).abs()));
                if v.1 < m.1 {
                    v_value *= multiplier;
                } else {
                    m_value *= multiplier;
                }
                remainder = v_value % m_value;
            }
            if remainder != 0.0 {
                let text = js::number_to_string(multiple_of);
                self.push(
                    n,
                    vr,
                    Msg::Owned(format!("Value is not divisible by {text}.")),
                );
            }
        }
        if let Some(limit) = c.exclusive_minimum {
            if val <= limit {
                let text = js::number_to_string(limit);
                self.push(
                    n,
                    vr,
                    Msg::Owned(format!("Value is below the exclusive minimum of {text}.")),
                );
            }
        }
        if let Some(limit) = c.exclusive_maximum {
            if val >= limit {
                let text = js::number_to_string(limit);
                self.push(
                    n,
                    vr,
                    Msg::Owned(format!("Value is above the exclusive maximum of {text}.")),
                );
            }
        }
        if let Some(limit) = c.minimum {
            if val < limit {
                let text = js::number_to_string(limit);
                self.push(
                    n,
                    vr,
                    Msg::Owned(format!("Value is below the minimum of {text}.")),
                );
            }
        }
        if let Some(limit) = c.maximum {
            if val > limit {
                let text = js::number_to_string(limit);
                self.push(
                    n,
                    vr,
                    Msg::Owned(format!("Value is above the maximum of {text}.")),
                );
            }
        }
    }

    fn validate_string(&self, n: u32, value: &str, c: &'a StrC, vr: &mut Vr<'a>) {
        if c.min_length.is_some() || c.max_length.is_some() {
            let length = value.chars().count() as f64;
            if let Some(limit) = c.min_length {
                if length < limit {
                    let text = js::number_to_string(limit);
                    self.push(
                        n,
                        vr,
                        Msg::Owned(format!(
                            "String is shorter than the minimum length of {text}."
                        )),
                    );
                }
            }
            if let Some(limit) = c.max_length {
                if length > limit {
                    let text = js::number_to_string(limit);
                    self.push(
                        n,
                        vr,
                        Msg::Owned(format!(
                            "String is longer than the maximum length of {text}."
                        )),
                    );
                }
            }
        }
        if let Some((regex, message)) = &c.pattern {
            if !regex.is_match(value) {
                self.push(n, vr, Msg::Str(message));
            }
        }
        if let Some((kind, specific)) = &c.format {
            match kind {
                FormatKind::Uri | FormatKind::UriReference => {
                    let error = if value.is_empty() {
                        Some("URI expected.")
                    } else if matches!(kind, FormatKind::Uri) && !has_uri_scheme(value) {
                        Some("URI with a scheme is expected.")
                    } else {
                        None
                    };
                    if let Some(error) = error {
                        let message = match specific {
                            Some(m) => Msg::Str(m),
                            None => Msg::Owned(format!("String is not a URI: {error}")),
                        };
                        self.push(n, vr, message);
                    }
                }
                FormatKind::Regex(i) => {
                    let (regex, default) = format_regex(*i);
                    if value.is_empty() || !regex.is_match(value) {
                        let message = match specific {
                            Some(m) => Msg::Str(m),
                            None => Msg::Str(default),
                        };
                        self.push(n, vr, message);
                    }
                }
            }
        }
    }

    fn validate_array(
        &self,
        n: u32,
        s: &'a Compiled,
        c: &'a ArrC,
        vr: &mut Vr<'a>,
        depth: u32,
        track: bool,
    ) -> R {
        let items = self.doc.items(n);
        let (prefix, additional) = if self.draft_2020 {
            (&c.prefix_2020, c.additional_2020.as_ref())
        } else {
            (&c.prefix_old, c.additional_old.as_ref())
        };
        let mark = |vr: &mut Vr<'a>, i: usize| {
            if track {
                vr.processed.insert(Cow::Owned(i.to_string()));
            }
        };
        let mut index = 0;
        match prefix {
            Prefix::Undefined | Prefix::NoLength => {}
            Prefix::Null => {
                return Err(JsError::type_error(
                    "Cannot read properties of null (reading 'length')",
                ))
            }
            Prefix::Items(list) => {
                let max = list.len().min(items.len());
                while index < max {
                    let mut r = vr.nested();
                    self.validate(
                        items[index],
                        as_schema(&list[index]),
                        &mut r,
                        depth + 1,
                        false,
                    )?;
                    vr.merge_property_match(r);
                    mark(vr, index);
                    index += 1;
                }
            }
            Prefix::Count(count) => {
                let max = (*count).min(items.len());
                while index < max {
                    vr.merge_property_match(vr.nested());
                    mark(vr, index);
                    index += 1;
                }
            }
        }
        if let Some(additional) = additional.filter(|_| index < items.len()) {
            if let J::Bool(b) = additional {
                if !*b {
                    self.push(
                        n,
                        vr,
                        Msg::Owned(format!(
                        "Array has too many items according to schema. Expected {index} or fewer."
                    )),
                    );
                }
                while index < items.len() {
                    mark(vr, index);
                    vr.properties_value_matches += 1;
                    index += 1;
                }
            } else {
                let sch = to_sch(additional);
                while index < items.len() {
                    let mut r = vr.nested();
                    self.validate(items[index], sch, &mut r, depth + 1, false)?;
                    vr.merge_property_match(r);
                    mark(vr, index);
                    index += 1;
                }
            }
        }
        if let Some(contains) = c.contains {
            let mut count = 0.0;
            for (i, &item) in items.iter().enumerate() {
                let mut r = Vr::quiet();
                self.validate(item, contains, &mut r, depth + 1, false)?;
                if !r.has_problems() {
                    count += 1.0;
                    if self.draft_2020 {
                        mark(vr, i);
                    }
                }
            }
            let own = |fallback: String| match &s.error_message {
                Some(m) => Msg::Str(m),
                None => Msg::Owned(fallback),
            };
            if count == 0.0 && c.min_contains.is_none() {
                self.push(n, vr, own("Array does not contain required item.".into()));
            }
            if let Some(limit) = c.min_contains {
                if count < limit {
                    let text = js::number_to_string(limit);
                    self.push(n, vr, own(format!(
                        "Array has too few items that match the contains contraint. Expected {text} or more."
                    )));
                }
            }
            if let Some(limit) = c.max_contains {
                if count > limit {
                    let text = js::number_to_string(limit);
                    self.push(n, vr, own(format!(
                        "Array has too many items that match the contains contraint. Expected {text} or less."
                    )));
                }
            }
        }
        if let Some(unevaluated) = &c.unevaluated {
            for (i, &item) in items.iter().enumerate() {
                if !vr.processed.contains(&i.to_string()) {
                    if matches!(unevaluated, J::Bool(false)) {
                        self.push(
                            n,
                            vr,
                            Msg::Str("Item does not match any validation rule from the array."),
                        );
                    } else {
                        let mut r = vr.nested();
                        self.validate(item, to_sch(unevaluated), &mut r, depth + 1, false)?;
                        vr.merge_property_match(r);
                    }
                }
                mark(vr, i);
                vr.properties_value_matches += 1;
            }
        }
        let length = items.len() as f64;
        if let Some(limit) = c.min_items {
            if length < limit {
                let text = js::number_to_string(limit);
                self.push(
                    n,
                    vr,
                    Msg::Owned(format!("Array has too few items. Expected {text} or more.")),
                );
            }
        }
        if let Some(limit) = c.max_items {
            if length > limit {
                let text = js::number_to_string(limit);
                self.push(
                    n,
                    vr,
                    Msg::Owned(format!(
                        "Array has too many items. Expected {text} or fewer."
                    )),
                );
            }
        }
        if c.unique_items {
            let duplicates = items
                .iter()
                .enumerate()
                .any(|(i, &a)| items[i + 1..].iter().any(|&b| self.equals_nodes(a, b)));
            if duplicates {
                self.push(n, vr, Msg::Str("Array has duplicate items."));
            }
        }
        Ok(())
    }

    fn not_allowed(&self, child: u32, name: &'a str, s: &'a Compiled, vr: &mut Vr<'a>) {
        let property = self.doc.nodes[child as usize].parent;
        let (key, _) = self.doc.property(property);
        let k = &self.doc.nodes[key as usize];
        let message = match &s.error_message {
            Some(m) => Msg::Str(m),
            None => Msg::NotAllowed(name),
        };
        vr.add(Problem {
            offset: k.offset,
            length: k.length,
            message,
            warning: false,
            enum_mismatch: false,
        });
    }

    fn apply_prop_sch(
        &self,
        child: u32,
        name: &'a str,
        schema: PropSch,
        s: &'a Compiled,
        vr: &mut Vr<'a>,
        depth: u32,
    ) -> R {
        match schema {
            PropSch::Bool(false) => self.not_allowed(child, name, s, vr),
            PropSch::Bool(true) => {
                vr.properties_matches += 1;
                vr.properties_value_matches += 1;
            }
            PropSch::Sch(sch) => {
                let mut r = vr.nested();
                self.validate(child, sch, &mut r, depth + 1, false)?;
                vr.merge_property_match(r);
            }
        }
        Ok(())
    }

    fn validate_object(
        &self,
        n: u32,
        s: &'a Compiled,
        c: &'a ObjC,
        vr: &mut Vr<'a>,
        depth: u32,
        track: bool,
    ) -> R {
        let doc = self.doc;
        let node = &doc.nodes[n as usize];
        let uniq = doc.uniq(n);
        for required in &c.required {
            if doc.seen(n, &required.key).is_none() {
                let message = required.message.as_ref().map_err(Clone::clone)?;
                let key = if node.parent != NONE {
                    match doc.nodes[node.parent as usize].kind {
                        Kind::Property { key, .. } => Some(key),
                        _ => None,
                    }
                } else {
                    None
                };
                let (offset, length) = match key {
                    Some(k) => (doc.nodes[k as usize].offset, doc.nodes[k as usize].length),
                    None => (node.offset, 1),
                };
                vr.add(Problem {
                    offset,
                    length,
                    message: Msg::Str(message),
                    warning: false,
                    enum_mismatch: false,
                });
            }
        }

        let wants_unprocessed =
            !c.pattern_properties.is_empty() || c.additional.is_some() || c.unevaluated.is_some();
        let mut unprocessed = Unprocessed::new(if wants_unprocessed { uniq.len() } else { 0 });

        for (name, schema) in &c.properties {
            if track {
                vr.processed.insert(Cow::Borrowed(name));
            }
            let Some(i) = doc.find(n, name) else { continue };
            if wants_unprocessed {
                unprocessed.delete(i);
            }
            let child = uniq[i].value;
            if child != NONE {
                self.apply_prop_sch(child, name, *schema, s, vr, depth)?;
            }
        }

        for (regex, schema) in &c.pattern_properties {
            let mut processed: Vec<usize> = Vec::new();
            for (i, u) in uniq.iter().enumerate() {
                if !unprocessed.has(i) {
                    continue;
                }
                let name = doc.str_value(u.key);
                if regex.is_match(name) {
                    processed.push(i);
                    if u.value != NONE {
                        self.apply_prop_sch(u.value, name, *schema, s, vr, depth)?;
                    }
                }
            }
            for i in processed {
                unprocessed.delete(i);
                if track {
                    vr.processed
                        .insert(Cow::Borrowed(doc.str_value(uniq[i].key)));
                }
            }
        }

        if let Some(additional) = c.additional {
            for (i, u) in uniq.iter().enumerate() {
                if !unprocessed.has(i) {
                    continue;
                }
                unprocessed.delete(i);
                let name = doc.str_value(u.key);
                if track {
                    vr.processed.insert(Cow::Borrowed(name));
                }
                if u.value != NONE {
                    match additional {
                        PropSch::Bool(false) => self.not_allowed(u.value, name, s, vr),
                        PropSch::Bool(true) => {}
                        PropSch::Sch(sch) => {
                            let mut r = vr.nested();
                            self.validate(u.value, sch, &mut r, depth + 1, false)?;
                            vr.merge_property_match(r);
                        }
                    }
                }
            }
        }

        if let Some(unevaluated) = c.unevaluated {
            let mut processed: Vec<usize> = Vec::new();
            for (i, u) in uniq.iter().enumerate() {
                if !unprocessed.has(i) {
                    continue;
                }
                let name = doc.str_value(u.key);
                if !vr.processed.contains(name) {
                    processed.push(i);
                    if u.value != NONE {
                        match unevaluated {
                            PropSch::Bool(false) => self.not_allowed(u.value, name, s, vr),
                            PropSch::Bool(true) => {}
                            PropSch::Sch(sch) => {
                                let mut r = vr.nested();
                                self.validate(u.value, sch, &mut r, depth + 1, false)?;
                                vr.merge_property_match(r);
                            }
                        }
                    }
                }
            }
            for i in processed {
                unprocessed.delete(i);
                if track {
                    vr.processed
                        .insert(Cow::Borrowed(doc.str_value(uniq[i].key)));
                }
            }
        }

        let property_count = doc.props(n).len() as f64;
        if let Some(limit) = c.max_properties {
            if property_count > limit {
                let text = js::number_to_string(limit);
                self.push(
                    n,
                    vr,
                    Msg::Owned(format!("Object has more properties than limit of {text}.")),
                );
            }
        }
        if let Some(limit) = c.min_properties {
            if property_count < limit {
                let text = js::number_to_string(limit);
                self.push(
                    n,
                    vr,
                    Msg::Owned(format!(
                        "Object has fewer properties than the required number of {text}"
                    )),
                );
            }
        }

        for list in [&c.dependent_required, &c.dependent_schemas, &c.dependencies] {
            for (key, dep) in list {
                if doc.seen(n, key).is_some() {
                    self.validate_dependencies(n, key, dep, vr, depth)?;
                }
            }
        }

        if let Some(names) = c.property_names {
            for &p in doc.props(n) {
                let (key, _) = doc.property(p);
                self.validate(key, names, vr, depth + 1, track)?;
            }
        }
        Ok(())
    }

    fn validate_dependencies(
        &self,
        n: u32,
        key: &str,
        dep: &'a Dep,
        vr: &mut Vr<'a>,
        depth: u32,
    ) -> R {
        match dep {
            Dep::Keys(required) => {
                for (name, _) in required {
                    if self.doc.seen(n, name).is_none() {
                        self.push(
                            n,
                            vr,
                            Msg::Owned(format!(
                                "Object is missing property {name} required by property {key}."
                            )),
                        );
                    } else {
                        vr.properties_value_matches += 1;
                    }
                }
            }
            Dep::Sch(Some(sch)) => {
                let mut r = vr.nested();
                self.validate(n, *sch, &mut r, depth + 1, false)?;
                vr.merge_property_match(r);
            }
            Dep::Sch(None) => {}
        }
        Ok(())
    }

    /// `equals(getNodeValue(node), value)`.
    fn equals_schema(&self, n: u32, value: &J) -> bool {
        match (&self.doc.nodes[n as usize].kind, value) {
            (Kind::Null, J::Null) => true,
            (Kind::Bool, J::Bool(b)) => self.doc.nodes[n as usize].bool_value == *b,
            (Kind::Number { value: a, .. }, J::Num(b)) => a == b,
            (Kind::String(a), J::Str(b)) => a.as_ref() == &**b,
            (Kind::Array { .. }, J::Arr(a)) => {
                let items = self.doc.items(n);
                let others = &self.arena.arrs[*a as usize];
                items.len() == others.len()
                    && items
                        .iter()
                        .zip(others)
                        .all(|(&i, o)| self.equals_schema(i, o))
            }
            (Kind::Object { .. }, J::Obj(o)) => {
                let fields = self.object_fields(n);
                let entries = &self.arena.objs[*o as usize].entries;
                fields.len() == entries.len()
                    && fields.iter().all(|(k, v)| {
                        self.arena
                            .get(*o, k)
                            .is_some_and(|other| self.equals_schema(*v, other))
                    })
            }
            _ => false,
        }
    }

    /// The own properties of `getNodeValue(object)`.
    fn object_fields(&self, n: u32) -> Vec<(&str, u32)> {
        let mut fields: Vec<(&str, u32)> = Vec::new();
        for &p in self.doc.props(n) {
            let (key, value) = self.doc.property(p);
            if value == NONE {
                continue;
            }
            let name = self.doc.str_value(key);
            match fields.iter_mut().find(|(k, _)| *k == name) {
                Some(slot) => slot.1 = value,
                None => fields.push((name, value)),
            }
        }
        fields
    }

    fn equals_nodes(&self, a: u32, b: u32) -> bool {
        match (
            &self.doc.nodes[a as usize].kind,
            &self.doc.nodes[b as usize].kind,
        ) {
            (Kind::Null, Kind::Null) => true,
            (Kind::Bool, Kind::Bool) => {
                self.doc.nodes[a as usize].bool_value == self.doc.nodes[b as usize].bool_value
            }
            (Kind::Number { value: x, .. }, Kind::Number { value: y, .. }) => x == y,
            (Kind::String(x), Kind::String(y)) => x == y,
            (Kind::Array { .. }, Kind::Array { .. }) => {
                let (x, y) = (self.doc.items(a), self.doc.items(b));
                x.len() == y.len() && x.iter().zip(y).all(|(&i, &j)| self.equals_nodes(i, j))
            }
            (Kind::Object { .. }, Kind::Object { .. }) => {
                let (x, y) = (self.object_fields(a), self.object_fields(b));
                x.len() == y.len()
                    && x.iter().all(|(k, v)| {
                        y.iter()
                            .find(|(o, _)| o == k)
                            .is_some_and(|(_, w)| self.equals_nodes(*v, *w))
                    })
            }
            _ => false,
        }
    }
}

/// `/^(([^:/?#]+?):)?.../.exec(value)[2]` being set.
fn has_uri_scheme(value: &str) -> bool {
    match value.find([':', '/', '?', '#']) {
        Some(i) => i > 0 && value.as_bytes()[i] == b':',
        None => false,
    }
}

/// `normalizeFloats`: `(value, multiplier)`.
fn normalize_floats(float: f64) -> Option<(f64, i32)> {
    let text = js::number_to_string(float);
    // /^(-?\d+)(?:\.(\d+))?(?:e([-+]\d+))?$/
    let (mantissa, exponent) = match text.split_once('e') {
        Some((m, e)) => (m, Some(e)),
        None => (text.as_str(), None),
    };
    let (int, frac) = match mantissa.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (mantissa, None),
    };
    let digits = int.strip_prefix('-').unwrap_or(int);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value: f64 = format!("{int}{}", frac.unwrap_or("")).parse().ok()?;
    let exp: i32 = exponent.map_or(0, |e| e.parse().unwrap_or(0));
    Some((value, frac.map_or(0, str::len) as i32 - exp))
}
