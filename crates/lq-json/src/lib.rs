//! JSON for `lq`: faithful ports of the JavaScript pieces upstream leans on.
//!
//! - [`jsonc`]: `jsonc-parser` 3.3.1 (scanner, fault-tolerant `parse`, `parseTree`).
//! - [`ast`]: theme-check's `toJSONNode` AST (`jsonc/parse.ts`) with byte locations.
//! - [`value`]: a JavaScript-flavoured value (`Object` keeps JS own-key order).
//! - [`uri`]: `vscode-uri` plus theme-check's `path.ts` helpers.
//! - [`js`]: `Number#toString`, `JSON.stringify` and friends.
//! - [`validator`]: `vscode-json-languageservice`'s validation, as `JSONValidator` uses it.
//!
//! All offsets are UTF-8 byte offsets into the source. Convert with
//! `lq_syntax::SourceMap` at reporting time.

pub mod ast;
pub mod js;
pub mod jsonc;
pub mod regex;
pub mod uri;
pub mod validator;
pub mod value;

pub use ast::{to_json_node, JsonNode, JsonParseFailure, Literal, Loc};
pub use jsonc::{parse, parse_json, parse_tree, ParseError, ParseErrorCode, ParseOptions};
pub use value::{Object, Value};
