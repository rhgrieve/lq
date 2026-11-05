//! Prettier's core options plus the plugin's, with their defaults.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HtmlWhitespaceSensitivity {
    Css,
    Strict,
    Ignore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureWhitespaceSensitivity {
    Strict,
    Ignore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndOfLine {
    Lf,
    Crlf,
    Cr,
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbeddedLanguageFormatting {
    Auto,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectWrap {
    Preserve,
    Collapse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrailingComma {
    All,
    Es5,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrowParens {
    Always,
    Avoid,
}

/// `consistent` has no Biome equivalent and behaves as `as-needed` in embedded JS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuoteProps {
    AsNeeded,
    Consistent,
    Preserve,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub print_width: usize,
    pub tab_width: usize,
    pub use_tabs: bool,
    pub single_quote: bool,
    pub bracket_same_line: bool,
    pub single_attribute_per_line: bool,
    pub html_whitespace_sensitivity: HtmlWhitespaceSensitivity,
    pub end_of_line: EndOfLine,
    pub embedded_language_formatting: EmbeddedLanguageFormatting,
    // JS/JSON options that reach embedded code.
    pub bracket_spacing: bool,
    pub object_wrap: ObjectWrap,
    pub semi: bool,
    pub trailing_comma: TrailingComma,
    pub arrow_parens: ArrowParens,
    pub jsx_single_quote: bool,
    pub quote_props: QuoteProps,
    // Plugin options.
    pub capture_whitespace_sensitivity: CaptureWhitespaceSensitivity,
    pub liquid_single_quote: bool,
    pub embedded_single_quote: bool,
    pub single_line_link_tags: bool,
    pub indent_schema: bool,
    pub liquid_doc_param_dash: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            // The plugin's `defaultOptions`; Prettier's own default is 80.
            print_width: 120,
            tab_width: 2,
            use_tabs: false,
            single_quote: false,
            bracket_same_line: false,
            single_attribute_per_line: false,
            html_whitespace_sensitivity: HtmlWhitespaceSensitivity::Css,
            end_of_line: EndOfLine::Lf,
            embedded_language_formatting: EmbeddedLanguageFormatting::Auto,
            bracket_spacing: true,
            object_wrap: ObjectWrap::Preserve,
            semi: true,
            trailing_comma: TrailingComma::All,
            arrow_parens: ArrowParens::Always,
            jsx_single_quote: false,
            quote_props: QuoteProps::AsNeeded,
            capture_whitespace_sensitivity: CaptureWhitespaceSensitivity::Strict,
            liquid_single_quote: true,
            embedded_single_quote: true,
            single_line_link_tags: false,
            indent_schema: false,
            liquid_doc_param_dash: true,
        }
    }
}
