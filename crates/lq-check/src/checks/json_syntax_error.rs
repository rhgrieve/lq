//! `JSONSyntaxError`: deprecated; a run with `validateJSON` uses `ValidJSON` instead.

use crate::api::*;
use lq_json::jsonc::ParseErrorCode;

pub const IMPL: CheckImpl = CheckImpl::Json { create, enter: JsonMask::NONE, exit: JsonMask::NONE };

fn create<'a, 'f>(cx: &JsonCx<'a, 'f>) -> Option<Box<dyn JsonCheck<'a> + 'a>> {
    if cx.has_validate_json() || cx.file().json_ast().is_ok() {
        return None;
    }
    Some(Box::new(JsonSyntaxError))
}

struct JsonSyntaxError;

impl<'a> JsonCheck<'a> for JsonSyntaxError {
    fn on_code_path_start(&mut self, cx: &JsonCx<'a, '_>) {
        // Upstream's other branch handles errors that aren't `JSONCParseErrors`;
        // `toJSONNode` throws nothing else.
        let Err(failure) = cx.file().json_ast() else { return };
        for error in &failure.errors {
            cx.report(JsonProblem::new(
                message(error.error),
                error.offset,
                error.offset + error.length,
            ));
        }
    }
}

/// `jsoncParseErrorMessage`.
fn message(code: ParseErrorCode) -> &'static str {
    match code {
        ParseErrorCode::InvalidSymbol => "Invalid symbol",
        ParseErrorCode::InvalidNumberFormat => "Invalid number format",
        ParseErrorCode::PropertyNameExpected => "Property name expected",
        ParseErrorCode::ValueExpected => "Expecting a value",
        ParseErrorCode::ColonExpected => "Expecting a colon after a property name (:)",
        ParseErrorCode::CommaExpected => "Expecting a comma",
        ParseErrorCode::CloseBraceExpected => "Expecting a closing brace (})",
        ParseErrorCode::CloseBracketExpected => "Expecting a closing bracket (])",
        ParseErrorCode::EndOfFileExpected => "Expecting end of file",
        ParseErrorCode::InvalidCommentToken => "Invalid comment token",
        ParseErrorCode::UnexpectedEndOfComment => "Unexpected end of comment",
        ParseErrorCode::UnexpectedEndOfString => "Unexpected end of string",
        ParseErrorCode::UnexpectedEndOfNumber => "Unexpected end of number",
        ParseErrorCode::InvalidUnicode => "Invalid unicode",
        ParseErrorCode::InvalidEscapeCharacter => "Invalid escape character",
        ParseErrorCode::InvalidCharacter => "Invalid character",
    }
}
