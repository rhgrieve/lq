//! Tag classification constants used by the parser and external consumers.

pub const TAGS_WITHOUT_MARKUP: &[&str] =
    &["style", "schema", "javascript", "else", "break", "continue", "comment", "raw", "doc"];

pub const BLOCKS: &[&str] = &[
    "if", "unless", "for", "case", "tablerow", "capture", "form", "paginate", "block", "ifchanged",
    "partial",
];

pub const RAW_TAGS: &[&str] =
    &["raw", "javascript", "schema", "stylesheet", "style", "comment", "doc"];

pub const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "command", "embed", "hr", "img", "input", "keygen", "link", "meta",
    "param", "source", "track", "wbr",
];

pub const HTML_RAW_TAGS: &[&str] = &["script", "style", "svg"];
