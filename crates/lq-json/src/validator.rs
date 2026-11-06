//! Port of the part of `vscode-json-languageservice` 5.7.2 behind theme-check's
//! `JSONValidator.ts`: `parseJSONDocument` and `doValidation`, with the same
//! messages, positions, ordering and alternative matching.

mod eval;
mod glob;
mod parser;
mod schema;
mod url;

use eval::{allows, root_draft_2020, Eval, Sch, Vr};
use parser::{Doc, Kind, NONE};
use schema::{normalize_id, Service, Target, J};
use std::collections::{HashMap, HashSet};
use std::sync::{OnceLock, RwLock};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaDefinition {
    pub uri: String,
    pub file_match: Option<Vec<String>>,
    pub schema: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// `start` and `end` are byte offsets into the validated text, after the
/// `positionAt`/`offsetAt` round trip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub message: String,
    pub start: u32,
    pub end: u32,
    pub severity: Severity,
}

/// A rejected promise upstream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsError {
    pub name: String,
    pub message: String,
}

impl JsError {
    fn type_error(message: &str) -> JsError {
        JsError {
            name: "TypeError".into(),
            message: message.into(),
        }
    }

    fn range_error() -> JsError {
        JsError {
            name: "RangeError".into(),
            message: "Maximum call stack size exceeded".into(),
        }
    }
}

struct State {
    service: RwLock<Service>,
}

/// `JSONValidator`. Nothing is parsed or resolved until a schema is first used.
pub struct JsonValidator {
    definitions: Vec<SchemaDefinition>,
    state: OnceLock<State>,
}

/// `positionAt` then `offsetAt`: clamped, and moved off the middle of a CRLF.
fn round_trip(text: &str, offset: u32) -> u32 {
    let b = text.as_bytes();
    let o = (offset as usize).min(b.len());
    if o > 0 && o < b.len() && b[o - 1] == b'\r' && b[o] == b'\n' {
        (o - 1) as u32
    } else {
        o as u32
    }
}

fn floor_char_boundary(text: &str, offset: u32) -> u32 {
    (0..=offset as usize)
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .map_or(offset, |i| i as u32)
}

/// `addProblem`: the signature's line and character are one position, and a
/// position is the round-tripped offset.
struct Collector<'t> {
    text: &'t str,
    out: Vec<Diagnostic>,
    added: HashSet<(u32, String)>,
}

impl Collector<'_> {
    fn add(&mut self, start: u32, end: u32, message: String, severity: Severity) {
        let start = round_trip(self.text, start);
        if self.added.insert((start, message.clone())) {
            let end = floor_char_boundary(self.text, round_trip(self.text, end));
            // A position inside an astral character (see `parser::Parser::error`).
            let start = floor_char_boundary(self.text, start);
            self.out.push(Diagnostic {
                message,
                start,
                end,
                severity,
            });
        }
    }
}

/// The first half of `addSchemaProblem`: where it reports.
fn schema_problem_range(doc: &Doc<'_>) -> (u32, u32) {
    let root = &doc.nodes[doc.root as usize];
    if matches!(root.kind, Kind::Object { .. }) {
        if let Some(&first) = doc.props(doc.root).first() {
            let (key, value) = doc.property(first);
            if doc.str_value(key) == "$schema" {
                let node = if value != NONE { value } else { first };
                let node = &doc.nodes[node as usize];
                return (node.offset, node.offset + node.length);
            }
        }
    }
    (root.offset, root.offset + 1)
}

impl JsonValidator {
    pub fn new(schemas: Vec<SchemaDefinition>) -> JsonValidator {
        JsonValidator {
            definitions: schemas,
            state: OnceLock::new(),
        }
    }

    fn state(&self) -> &State {
        self.state.get_or_init(|| State {
            service: RwLock::new(Service::new(&self.definitions)),
        })
    }

    /// `JSONValidator.validate`.
    pub fn validate(&self, uri: &str, json: &str) -> Result<Vec<Diagnostic>, JsError> {
        let doc = parser::parse(json)?;
        self.do_validation(uri, &doc)
    }

    /// `isValid`: every diagnostic's severity is something other than error.
    pub fn is_valid(&self, uri: &str, json: &str) -> Result<bool, JsError> {
        Ok(self
            .validate(uri, json)?
            .iter()
            .all(|d| d.severity != Severity::Error))
    }

    /// `getSchemaFromProperty`.
    fn schema_from_property(
        &self,
        resource: &str,
        doc: &Doc<'_>,
    ) -> Result<Option<String>, JsError> {
        if doc.root == NONE || !matches!(doc.nodes[doc.root as usize].kind, Kind::Object { .. }) {
            return Ok(None);
        }
        for &p in doc.props(doc.root) {
            let (key, value) = doc.property(p);
            if doc.str_value(key) != "$schema" || value == NONE {
                continue;
            }
            if let Kind::String(id) = &doc.nodes[value as usize].kind {
                if has_scheme(id) {
                    return Ok(Some(id.to_string()));
                }
                let resolved =
                    url::resolve(id, resource).map_err(|()| JsError::type_error("Invalid URL"))?;
                return Ok(Some(resolved));
            }
        }
        Ok(None)
    }

    fn do_validation(&self, uri: &str, doc: &Doc<'_>) -> Result<Vec<Diagnostic>, JsError> {
        let state = self.state();
        let from_property = self.schema_from_property(uri, doc)?;
        let is_property = from_property.is_some();
        let lock_read = || state.service.read().unwrap_or_else(|e| e.into_inner());
        let target = match from_property {
            Some(id) => Target::Id(normalize_id(&id)),
            None => {
                let service = lock_read();
                let ids = service.associated(uri);
                match ids.len() {
                    0 => Target::None,
                    1 => Target::Id(ids[0].to_string()),
                    _ => Target::Combined(ids.iter().map(|s| s.to_string()).collect()),
                }
            }
        };
        let ready = lock_read().ready_handle(&target);
        let handle = match ready {
            Some(handle) => handle,
            None => {
                let definitions: HashMap<&str, &str> = self
                    .definitions
                    .iter()
                    .map(|d| (d.uri.as_str(), d.schema.as_str()))
                    .collect();
                let mut service = state.service.write().unwrap_or_else(|e| e.into_inner());
                service.resolve_target(&target, uri, is_property, &definitions)
            }
        };
        let service = lock_read();
        let mut collector = Collector {
            text: doc.text,
            out: Vec::new(),
            added: HashSet::new(),
        };
        if let Some(handle) = handle {
            let resolved = match &service.handles[handle as usize].resolved {
                Some(Ok(resolved)) => resolved,
                Some(Err(e)) => return Err(e.clone()),
                None => return Ok(Vec::new()),
            };
            if let Some(error) = &resolved.first_error {
                if doc.root != NONE {
                    let (start, end) = schema_problem_range(doc);
                    collector.add(start, end, error.clone(), Severity::Warning);
                }
            } else {
                for warning in &resolved.warnings {
                    if doc.root != NONE {
                        let (start, end) = schema_problem_range(doc);
                        collector.add(start, end, warning.clone(), Severity::Warning);
                    }
                }
                let root = resolved.root.as_ref().filter(|r| r.truthy());
                if let (true, Some(root)) = (doc.root != NONE, root) {
                    self.semantic(doc, &service, root, &mut collector)?;
                }
            }
            if let Some(root) = &resolved.root {
                resolved
                    .allow_check
                    .get_or_init(|| {
                        allows(&service.arena, root, &["allowComments"], 0)?;
                        allows(
                            &service.arena,
                            root,
                            &["allowTrailingCommas", "allowsTrailingCommas"],
                            0,
                        )?;
                        Ok(())
                    })
                    .clone()?;
            }
        }
        for p in &doc.problems {
            if !p.trailing_comma {
                collector.add(p.start, p.end, p.message.to_string(), p.severity);
            }
        }
        Ok(collector.out)
    }

    /// `JSONDocument.validate`.
    fn semantic(
        &self,
        doc: &Doc<'_>,
        service: &Service,
        root: &J,
        collector: &mut Collector<'_>,
    ) -> Result<(), JsError> {
        let draft_2020 = root_draft_2020(&service.arena, root)?;
        let eval = Eval {
            doc,
            arena: &service.arena,
            draft_2020,
        };
        let mut vr = Vr::default();
        let sch = match root {
            J::Obj(o) => Sch::Obj(*o),
            _ => Sch::Obj(schema::EMPTY_OBJECT),
        };
        eval.validate(doc.root, sch, &mut vr, 0, false)?;
        for p in &vr.problems {
            let severity = if p.warning {
                Severity::Warning
            } else {
                Severity::Error
            };
            collector.add(
                p.offset,
                p.offset + p.length,
                p.message.render(&service.arena),
                severity,
            );
        }
        Ok(())
    }
}

/// `/^\w[\w\d+.-]*:/`.
fn has_scheme(id: &str) -> bool {
    let b = id.as_bytes();
    let word = |c: &u8| c.is_ascii_alphanumeric() || *c == b'_';
    if !b.first().is_some_and(word) {
        return false;
    }
    match b
        .iter()
        .position(|c| !(word(c) || matches!(c, b'+' | b'.' | b'-')))
    {
        Some(i) => b[i] == b':',
        None => false,
    }
}

/// `new URL(input, base).toString()`, for the differential harness.
#[doc(hidden)]
pub fn resolve_url_for_debug(input: &str, base: &str) -> Option<String> {
    url::resolve(input, base).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "https://example.com/schemas";

    fn validator(files: &[(&str, &str, &[&str])]) -> JsonValidator {
        JsonValidator::new(
            files
                .iter()
                .map(|(name, schema, globs)| SchemaDefinition {
                    uri: format!("{ROOT}/{name}"),
                    file_match: (!globs.is_empty())
                        .then(|| globs.iter().map(|g| g.to_string()).collect()),
                    schema: schema.to_string(),
                })
                .collect(),
        )
    }

    /// `(message, start, end)` for one schema matching `t/*.json`.
    fn run(schema: &str, json: &str) -> Vec<(String, u32, u32)> {
        validator(&[("s.json", schema, &["t/*.json"])])
            .validate("file:///w/t/doc.json", json)
            .unwrap()
            .into_iter()
            .map(|d| (d.message, d.start, d.end))
            .collect()
    }

    fn messages(schema: &str, json: &str) -> Vec<String> {
        run(schema, json).into_iter().map(|d| d.0).collect()
    }

    #[test]
    fn validator_is_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<JsonValidator>();
    }

    #[test]
    fn enum_messages_merge_across_equally_good_alternatives() {
        let schema = r#"{"anyOf":[{"enum":["a","b"]},{"enum":["c"]},{"const":"d"}]}"#;
        assert_eq!(
            run(schema, r#""x""#),
            [(
                r#"Value is not accepted. Valid values: "a", "b", "c", "d"."#.to_string(),
                0,
                3
            )]
        );
        assert!(run(schema, r#""c""#).is_empty());
    }

    #[test]
    fn one_of_reports_multiple_matches_on_the_first_character() {
        let schema = r#"{"oneOf":[{"type":"object"},{"required":[]}]}"#;
        assert_eq!(
            run(schema, "{}"),
            [(
                "Matches multiple schemas when only one must validate.".to_string(),
                0,
                1
            )]
        );
    }

    #[test]
    fn best_match_prefers_the_alternative_with_matching_properties() {
        let schema = r#"{"oneOf":[
            {"required":["a"],"properties":{"a":{"const":1},"b":{"type":"string"}}},
            {"properties":{"c":{"const":2},"d":{"type":"string"}}}]}"#;
        assert_eq!(
            messages(schema, r#"{"c":2,"d":5}"#),
            [r#"Incorrect type. Expected "string"."#]
        );
    }

    #[test]
    fn discriminator_only_tries_the_alternative_it_selects() {
        let schema = r#"{"oneOf":[
            {"properties":{"kind":{"const":"a"}},"required":["x"]},
            {"properties":{"kind":{"const":"b"}},"required":["y"]}]}"#;
        assert_eq!(
            messages(schema, r#"{"kind":"b"}"#),
            [r#"Missing property "y"."#]
        );
        // No alternative for the value: all of them run, and the first stays best.
        assert_eq!(
            messages(schema, r#"{"kind":"z"}"#),
            [r#"Missing property "x"."#, r#"Value must be "a"."#]
        );
    }

    #[test]
    fn if_then_else() {
        let schema =
            r#"{"if":{"required":["a"]},"then":{"required":["b"]},"else":{"required":["c"]}}"#;
        assert_eq!(messages(schema, r#"{"a":1}"#), [r#"Missing property "b"."#]);
        assert_eq!(messages(schema, "{}"), [r#"Missing property "c"."#]);
    }

    #[test]
    fn multiple_of_with_floats() {
        let schema = r#"{"multipleOf":0.1}"#;
        assert!(messages(schema, "0.3").is_empty());
        assert!(messages(schema, "1.1").is_empty());
        assert_eq!(messages(schema, "0.35"), ["Value is not divisible by 0.1."]);
    }

    #[test]
    fn string_length_counts_code_points() {
        let schema = r#"{"maxLength":1}"#;
        assert!(messages(schema, r#""😀""#).is_empty());
        assert_eq!(
            messages(schema, r#""😀😀""#),
            ["String is longer than the maximum length of 1."]
        );
    }

    #[test]
    fn schema_authored_messages() {
        let schema =
            r#"{"type":"string","pattern":"^a","patternErrorMessage":"P","errorMessage":"E"}"#;
        assert_eq!(messages(schema, "1"), ["E"]);
        assert_eq!(messages(schema, r#""b""#), ["P"]);
    }

    #[test]
    fn deprecation_is_a_warning_on_the_whole_property() {
        let v = validator(&[(
            "s.json",
            r#"{"properties":{"a":{"deprecationMessage":"old"}}}"#,
            &["*.json"],
        )]);
        let d = v.validate("file:///x.json", r#"{"a":1}"#).unwrap();
        assert_eq!(
            d,
            [Diagnostic {
                message: "old".into(),
                start: 1,
                end: 6,
                severity: Severity::Warning
            }]
        );
        assert!(v.is_valid("file:///x.json", r#"{"a":1}"#).unwrap());
    }

    #[test]
    fn syntax_errors_follow_schema_problems_and_trailing_commas_are_ignored() {
        assert_eq!(
            messages(r#"{"required":["a"]}"#, r#"{"b":1,}"#),
            [r#"Missing property "a"."#]
        );
        assert_eq!(
            run(r#"{"type":"array"}"#, r#"{"a":1"#),
            [
                (r#"Incorrect type. Expected "array"."#.to_string(), 0, 6),
                ("Expected comma or closing brace".to_string(), 5, 6)
            ]
        );
    }

    #[test]
    fn duplicate_keys_warn_on_both() {
        let d = validator(&[])
            .validate("file:///x.json", r#"{"a":1,"a":2,"a":3}"#)
            .unwrap();
        let starts: Vec<u32> = d.iter().map(|d| d.start).collect();
        assert_eq!(starts, [7, 1, 13]);
        assert!(d.iter().all(|d| d.severity == Severity::Warning));
    }

    #[test]
    fn offsets_are_bytes_and_avoid_the_middle_of_crlf() {
        let d = validator(&[])
            .validate("file:///x.json", "{\"é\":1,\"é\":2}")
            .unwrap();
        assert_eq!((d[0].start, d[0].end), (8, 12));
        // The problem for `[` at the end of a line, whose range ends on the `\n`.
        let d = validator(&[])
            .validate("file:///x.json", "[1,\r\n")
            .unwrap();
        assert_eq!((d[0].start, d[0].end), (2, 3));
    }

    #[test]
    fn an_astral_character_before_the_end_is_one_position() {
        let d = validator(&[])
            .validate("file:///x.json", "{\"a\":😀")
            .unwrap();
        assert_eq!(
            d.iter().map(|d| (d.start, d.end)).collect::<Vec<_>>(),
            [(5, 9), (5, 9)]
        );
        assert_eq!(d[1].message, "Expected comma or closing brace");
    }

    #[test]
    fn refs_across_files_and_unresolvable_ones() {
        let v = validator(&[
            (
                "a.json",
                r##"{"properties":{"p":{"$ref":"./b.json#/definitions/d"}}}"##,
                &["t/*.json"],
            ),
            ("b.json", r#"{"definitions":{"d":{"type":"string"}}}"#, &[]),
            ("c.json", r##"{"$ref":"#/nope"}"##, &["u/*.json"]),
        ]);
        let d = v.validate("file:///t/x.json", r#"{"p":1}"#).unwrap();
        assert_eq!(d[0].message, r#"Incorrect type. Expected "string"."#);
        assert_eq!((d[0].start, d[0].end), (5, 6));
        let d = v.validate("file:///u/x.json", "{}").unwrap();
        assert_eq!(
            d[0].message,
            format!("$ref '/nope' in '{ROOT}/c.json' can not be resolved.")
        );
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!((d[0].start, d[0].end), (0, 1));
    }

    #[test]
    fn a_merge_sees_a_section_whose_external_ref_was_already_taken() {
        // `additionalProperties` is walked after `definitions`, so its merge copies a
        // `late` that has lost its `$ref` and not yet received the external content.
        let v = validator(&[
            (
                "a.json",
                r##"{"definitions":{"late":{"$ref":"./b.json"}},"additionalProperties":{"$ref":"#/definitions/late"},"properties":{"p":{"$ref":"#/definitions/late"}}}"##,
                &["*.json"],
            ),
            ("b.json", r#"{"type":"string"}"#, &[]),
        ]);
        let d = v.validate("file:///x.json", r#"{"p":1,"q":1}"#).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!((d[0].start, d[0].end), (5, 6));
    }

    #[test]
    fn dollar_schema_in_the_document() {
        let v = validator(&[("a.json", r#"{"required":["a"]}"#, &[])]);
        let d = v
            .validate(
                "file:///x.json",
                &format!(r#"{{"$schema":"{ROOT}/a.json"}}"#),
            )
            .unwrap();
        assert_eq!(d[0].message, r#"Missing property "a"."#);
        let e = v
            .validate(
                "file:///x.json",
                r#"{"$schema":"https://example.com/nope"}"#,
            )
            .unwrap_err();
        assert_eq!(e.name, "TypeError");
        assert_eq!(
            e.message,
            "Cannot read properties of undefined (reading '$schema')"
        );
        let e = v
            .validate("not a uri", r#"{"$schema":"relative.json"}"#)
            .unwrap_err();
        assert_eq!(
            (e.name.as_str(), e.message.as_str()),
            ("TypeError", "Invalid URL")
        );
        // A built-in meta schema.
        let d = v
            .validate(
                "file:///x.json",
                r#"{"$schema":"http://json-schema.org/draft-07/schema#","type":5}"#,
            )
            .unwrap();
        assert!(!d.is_empty());
    }

    #[test]
    fn schema_shapes_that_throw() {
        let e = validator(&[("a.json", "null", &["*.json"])])
            .validate("file:///x.json", "1")
            .unwrap_err();
        assert_eq!(
            e.message,
            "Cannot read properties of null (reading '$schema')"
        );
        let e = validator(&[("a.json", r#"{"allOf":[null]}"#, &["*.json"])])
            .validate("file:///x.json", "1")
            .unwrap_err();
        assert_eq!(e.message, "Cannot read properties of null (reading 'type')");
        let e = validator(&[("a.json", r##"{"allOf":[{"$ref":"#"}]}"##, &["*.json"])])
            .validate("file:///x.json", "1")
            .unwrap_err();
        assert_eq!(
            (e.name.as_str(), e.message.as_str()),
            ("RangeError", "Maximum call stack size exceeded")
        );
    }

    #[test]
    fn external_ref_cycles_do_not_hang() {
        // `x` is no schema keyword, so the sections keep their `$ref`s. Each merge brings
        // the next one back, and upstream's promise never settles.
        let v = validator(&[
            ("a.json", r##"{"$ref":"./b.json#/x/y"}"##, &["*.json"]),
            ("b.json", r##"{"x":{"y":{"$ref":"./c.json#/x/y"}}}"##, &[]),
            ("c.json", r##"{"x":{"y":{"$ref":"./b.json#/x/y"}}}"##, &[]),
        ]);
        assert!(v.validate("file:///x.json", "1").is_err());
    }

    #[test]
    fn deep_documents_on_a_small_stack() {
        let doc = format!("{}{}", "[".repeat(2000), "]".repeat(2000));
        let v = validator(&[("a.json", r##"{"items":{"$ref":"#"}}"##, &["*.json"])]);
        // Unoptimized frames take over 128 KB before the first hop, and more on x64 Windows.
        std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(move || {
                assert!(v.validate("file:///x.json", &doc).unwrap().is_empty());
                let too_deep = "[".repeat(100_000);
                assert_eq!(
                    v.validate("file:///x.json", &too_deep).unwrap_err().name,
                    "RangeError"
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn concurrent_first_use() {
        let v = validator(&[
            (
                "a.json",
                r#"{"allOf":[{"$ref":"./b.json"}]}"#,
                &["a/*.json"],
            ),
            ("b.json", r#"{"required":["x"]}"#, &["b/*.json"]),
        ]);
        std::thread::scope(|s| {
            for i in 0..8 {
                let v = &v;
                s.spawn(move || {
                    let uri = if i % 2 == 0 {
                        "file:///a/d.json"
                    } else {
                        "file:///b/d.json"
                    };
                    let d = v.validate(uri, "{}").unwrap();
                    assert_eq!(d[0].message, r#"Missing property "x"."#);
                });
            }
        });
    }
}
