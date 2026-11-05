//! Port of `printer/printer-liquid-html.ts`.

use super::string::*;
use super::utils::*;
use super::*;
use crate::concat;
use crate::doc::utils::{remove_lines, replace_end_of_line};
use crate::doc::{
    dedent_to_root, group, group_id, indent, join_hardline, join_with, Doc,
};
use crate::options::EmbeddedLanguageFormatting;
use lq_syntax::{get_conditional_comment, Node, RawMarkupKind};

fn comma_line<'a>() -> Doc<'a> {
    concat![",", Doc::Line]
}

/// `/^[a-z0-9_]+\??$/i`.
fn is_simple_lookup(value: &str) -> bool {
    let body = value.strip_suffix('?').unwrap_or(value);
    !body.is_empty() && body.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// `/^---\r?\n/`.
fn starts_with_frontmatter_marker(value: &str) -> bool {
    value.strip_prefix("---").is_some_and(|r| r.starts_with('\n') || r.starts_with("\r\n"))
}

impl<'a> Ctx<'a> {
    fn print_attribute_name(&self, name: &'a [Node<'a>]) -> Doc<'a> {
        Doc::Concat(
            name.iter().map(|part| remove_lines(self.print_node(part, Args::default()))).collect(),
        )
    }

    fn print_attribute(&self, node: N<'a>) -> Doc<'a> {
        let (Node::AttrSingleQuoted(attr) | Node::AttrDoubleQuoted(attr) | Node::AttrUnquoted(attr)) =
            node
        else {
            return Doc::Nil;
        };
        let attr_group_id = self.symbol();
        let (start, end) = (attr.attribute_position.start as usize, attr.attribute_position.end as usize);
        let value = slice(self.source, start, end);
        let preferred_quote = if self.options.single_quote { "'" } else { "\"" };
        let contains_quote = attr
            .value
            .iter()
            .any(|v| matches!(v, Node::TextNode(t) if t.value.contains(preferred_quote)));
        let quote = if contains_quote {
            if preferred_quote == "'" {
                "\""
            } else {
                "'"
            }
        } else {
            preferred_quote
        };

        concat![
            self.print_attribute_name(&attr.name),
            "=",
            quote,
            if has_line_break_in_range(self.source, start, end) {
                group_id(
                    concat![
                        indent(concat![
                            Doc::Softline,
                            join_hardline(&reindent(&body_lines(value), true)),
                        ]),
                        Doc::Softline,
                    ],
                    attr_group_id,
                )
            } else {
                value.into()
            },
            quote,
        ]
    }

    fn print_text_node(&self, node: N<'a>) -> Doc<'a> {
        let Node::TextNode(text_node) = node else {
            return Doc::Nil;
        };
        let text: &'a str = &text_node.value;

        if self.parent(node).is_some_and(|p| matches!(p, Node::Document(_)))
            && self.prev(node).is_none()
            && starts_with_frontmatter_marker(text)
        {
            return text.into();
        }

        if text.chars().all(is_js_ws) {
            return Doc::Nil;
        }

        if self.is_pre_like_node(node) {
            return concat![
                self.print_opening_tag_prefix(node),
                replace_end_of_line(text),
                self.print_closing_tag_suffix(node),
            ];
        }

        concat![
            self.print_opening_tag_prefix(node),
            join_with(paragraphs(text).into_iter().map(paragraph_fill), || Doc::Hardline),
            self.print_closing_tag_suffix(node),
        ]
    }

    /// The `RawMarkup` case of `printNode`, after `embed` declined or failed.
    fn print_raw_markup(&self, node: N<'a>) -> Doc<'a> {
        let Node::RawMarkup(raw) = node else {
            return Doc::Nil;
        };
        let parent = self.parent(node);

        if parent.and_then(string_name) == Some("doc") {
            return self.print_liquid_doc(&raw.nodes);
        }

        if matches!(raw.kind, RawMarkupKind::Typescript | RawMarkupKind::Javascript)
            && raw.value.contains('`')
        {
            return concat![js_trim_end(raw.value), Doc::Hardline];
        }

        let lines = body_lines(raw.value);
        let raw_first_line_isnt_indented = raw
            .value
            .split('\n')
            .next()
            .is_some_and(|l| l.strip_suffix('\r').unwrap_or(l).chars().any(|c| !is_js_ws(c)));

        let body = if lines.iter().any(|l| !js_trim(l).is_empty()) {
            join_hardline(&reindent(&lines, raw_first_line_isnt_indented))
        } else {
            Doc::Softline
        };

        if self.should_indent_body(node) {
            concat![indent(concat![Doc::Hardline, body]), Doc::Hardline]
        } else {
            concat![dedent_to_root(concat![Doc::Hardline, body]), Doc::Hardline]
        }
    }

    /// `shouldIndentBody` of embed.ts, equal to the inline check in `printNode`.
    pub fn should_indent_body(&self, node: N<'a>) -> bool {
        let Node::RawMarkup(raw) = node else {
            return false;
        };
        let should_not_indent_body = matches!(self.parent(node), Some(Node::LiquidRawTag(t)) if t.name == "schema")
            && !self.options.indent_schema;
        raw.kind != RawMarkupKind::Markdown && !should_not_indent_body
    }

    /// `printNode`.
    pub fn print_node(&self, node: N<'a>, args: Args) -> Doc<'a> {
        match node {
            Node::BlockArrayLiteral(n) => concat![
                "[",
                join_with(
                    n.elements.iter().map(|e| self.print_node(e, Args::default())),
                    comma_line
                ),
                "]",
            ],

            Node::Document(_) => concat![self.print_children(node, args), Doc::Hardline],

            Node::HtmlElement(_)
            | Node::HtmlDanglingMarkerClose(_)
            | Node::HtmlVoidElement(_)
            | Node::HtmlSelfClosingElement(_)
            | Node::HtmlRawNode(_) => self.print_element(node),

            Node::RawMarkup(_) => {
                let plain = self.print_raw_markup(node);
                if self.options.embedded_language_formatting == EmbeddedLanguageFormatting::Auto {
                    self.embed(node, plain)
                } else {
                    plain
                }
            }

            Node::LiquidVariableOutput(_) => self.print_liquid_variable_output(node, args),
            Node::LiquidRawTag(_) => self.print_liquid_raw_tag(node, args),
            Node::LiquidTag(_) => self.print_liquid_tag(node, args),
            Node::LiquidBranch(_) => self.print_liquid_branch(node, args),

            Node::AttrEmpty(n) => self.print_attribute_name(&n.name),
            Node::AttrUnquoted(_) | Node::AttrSingleQuoted(_) | Node::AttrDoubleQuoted(_) => {
                self.print_attribute(node)
            }

            Node::HtmlDoctype(n) => match n.legacy_doctype_string {
                Some(s) if !s.is_empty() => self.source_of(node).into(),
                _ => "<!doctype html>".into(),
            },

            Node::HtmlComment(n) => {
                let src = self.source_of(node);
                if let Some(c) = get_conditional_comment(src) {
                    return concat![
                        c.start_tag,
                        group(concat![
                            indent(concat![
                                Doc::Line,
                                join_hardline(&reindent(&body_lines(c.body), true)),
                            ]),
                            Doc::Line,
                        ]),
                        c.end_tag,
                    ];
                }
                if n.body.contains("prettier-ignore")
                    || n.body.starts_with("display:")
                    || n.body.starts_with("white-space:")
                {
                    return src.into();
                }
                concat![
                    "<!--",
                    group(concat![
                        indent(concat![
                            Doc::Line,
                            join_hardline(&reindent(&body_lines(n.body), true)),
                        ]),
                        Doc::Line,
                    ]),
                    "-->",
                ]
            }

            Node::AssignMarkup(n) => {
                concat![n.name, " = ", self.print_node(&n.value, Args::default())]
            }

            Node::CycleMarkup(n) => {
                let mut doc = Vec::with_capacity(4);
                if let Some(g) = &n.group_name {
                    doc.push(self.print_node(g, Args::default()));
                    doc.push(":".into());
                }
                let many = n.args.len() > 1;
                let whitespace = || if many { Doc::Line } else { " ".into() };
                doc.push(whitespace());
                doc.push(join_with(self.print_all(&n.args), || concat![",", whitespace()]));
                Doc::Concat(doc)
            }

            Node::ForMarkup(n) => {
                let mut doc: Vec<Doc<'a>> = vec![
                    n.variable_name.into(),
                    " in ".into(),
                    self.print_node(&n.collection, Args::default()),
                ];
                if n.reversed {
                    doc.push(Doc::Line);
                    doc.push("reversed".into());
                }
                if !n.args.is_empty() {
                    doc.push(concat![Doc::Line, join_with(self.print_all(&n.args), || Doc::Line)]);
                }
                Doc::Concat(doc)
            }

            Node::PaginateMarkup(n) => {
                let mut doc: Vec<Doc<'a>> = vec![
                    self.print_node(&n.collection, Args::default()),
                    Doc::Line,
                    "by ".into(),
                    self.print_node(&n.page_size, Args::default()),
                ];
                if !n.args.is_empty() {
                    doc.push(concat![
                        ",",
                        Doc::Line,
                        join_with(self.print_all(&n.args), comma_line),
                    ]);
                }
                Doc::Concat(doc)
            }

            Node::ContentForMarkup(n) => self.print_name_and_args(&n.content_for_type, &n.args),
            Node::SectionMarkup(n) => self.print_name_and_args(&n.name, &n.args),

            Node::BlockMarkup(n) => {
                let name = self.print_node(&n.name, Args::default());
                if n.args.is_empty() {
                    return name;
                }
                concat![
                    name,
                    ",",
                    Doc::Line,
                    join_with(self.print_all(&n.args), comma_line),
                    crate::doc::if_break(",", Doc::Nil, 0),
                ]
            }

            Node::RenderMarkup(n) => {
                let mut doc = vec![self.print_node(&n.snippet, Args::default())];
                let alias = n.alias.as_ref().and_then(|a| match a {
                    Node::RenderAliasExpression(a) if !a.value.is_empty() => Some(a.value),
                    _ => None,
                });
                if let Some(v) = &n.variable {
                    doc.push(if alias.is_some() { Doc::Line } else { " ".into() });
                    doc.push(self.print_node(v, Args::default()));
                }
                if let Some(a) = alias {
                    doc.push(" as ".into());
                    doc.push(a.into());
                }
                if !n.args.is_empty() {
                    doc.push(",".into());
                    doc.push(Doc::Line);
                    doc.push(join_with(self.print_all(&n.args), comma_line));
                }
                Doc::Concat(doc)
            }

            Node::RenderVariableExpression(n) => {
                concat![n.kind.as_str(), " ", self.print_node(&n.name, Args::default())]
            }

            Node::RenderAliasExpression(n) => n.value.into(),

            Node::LogicalExpression(n) => concat![
                self.print_node(&n.left, Args::default()),
                Doc::Line,
                n.relation.as_str(),
                " ",
                self.print_node(&n.right, Args::default()),
            ],

            Node::Comparison(n) => group(concat![
                self.print_node(&n.left, Args::default()),
                indent(concat![
                    Doc::Line,
                    n.comparator,
                    " ",
                    self.print_node(&n.right, Args::default()),
                ]),
            ]),

            Node::LiquidVariable(n) => {
                let name = self.print_node(&n.expression, Args::default());
                if n.filters.is_empty() {
                    return name;
                }
                concat![
                    name,
                    Doc::Line,
                    join_with(self.print_all(&n.filters), || Doc::Line),
                ]
            }

            Node::LiquidFilter(n) => {
                let mut parts: Vec<Doc<'a>> = vec!["| ".into(), n.name.into()];
                if !n.args.is_empty() {
                    let mut printed = self.print_all(&n.args);
                    if !matches!(n.args[0], Node::NamedArgument(_)) {
                        let rest = printed.split_off(1);
                        let first = printed.pop().unwrap_or(Doc::Nil);
                        parts.push(": ".into());
                        parts.push(first);
                        if !rest.is_empty() {
                            parts.push(indent(concat![
                                ",",
                                Doc::Line,
                                join_with(rest, comma_line),
                            ]));
                        }
                    } else {
                        parts.push(":".into());
                        parts.push(indent(concat![Doc::Line, join_with(printed, comma_line)]));
                    }
                }
                group(parts)
            }

            Node::NamedArgument(n) => {
                concat![&*n.name, ": ", self.print_node(&n.value, Args::default())]
            }

            Node::TextNode(_) => self.print_text_node(node),

            Node::YamlFrontmatter(n) => concat!["---", Doc::Hardline, n.body, "---"],

            Node::BooleanExpression(n) => self.print_node(&n.condition, Args::default()),

            Node::String(n) => {
                let preferred = if self.options.liquid_single_quote { "'" } else { "\"" };
                let quote = if n.value.contains(preferred) {
                    if preferred == "'" {
                        "\""
                    } else {
                        "'"
                    }
                } else {
                    preferred
                };
                concat![quote, n.value, quote]
            }

            Node::Number(n) => {
                if args.truncate {
                    strip_decimal_suffix(n.value).into()
                } else {
                    n.value.into()
                }
            }

            Node::Range(n) => {
                let truncate = Args { truncate: true, ..Args::default() };
                concat![
                    "(",
                    self.print_node(&n.start, truncate),
                    "..",
                    self.print_node(&n.end, truncate),
                    ")",
                ]
            }

            Node::LiquidLiteral(n) => {
                if n.keyword == "nil" {
                    "null".into()
                } else {
                    n.keyword.into()
                }
            }

            Node::VariableLookup(n) => {
                let mut doc = Vec::with_capacity(n.lookups.len() + 1);
                if let Some(name) = n.name.filter(|s| !s.is_empty()) {
                    doc.push(name.into());
                }
                for (index, lookup) in n.lookups.iter().enumerate() {
                    if let Node::String(s) = lookup {
                        let is_global_string_lookup = index == 0 && n.name.is_none_or(str::is_empty);
                        let starts_non_digit =
                            s.value.chars().next().is_some_and(|c| !c.is_ascii_digit());
                        if !is_global_string_lookup && starts_non_digit && is_simple_lookup(s.value) {
                            doc.push(concat![".", s.value]);
                            continue;
                        }
                    }
                    doc.push(concat!["[", self.print_node(lookup, Args::default()), "]"]);
                }
                Doc::Concat(doc)
            }

            Node::LiquidDocParamNode(_) => self.print_liquid_doc_param(node),
            Node::LiquidDocExampleNode(_) => self.print_liquid_doc_example(node),
            Node::LiquidDocDescriptionNode(_) => self.print_liquid_doc_description(node),
            Node::LiquidDocPromptNode(_) => self.print_liquid_doc_prompt(node),

            Node::LiquidErrorNode(_) => self.source_of(node).into(),
        }
    }

    fn print_all(&self, nodes: &'a [Node<'a>]) -> Vec<Doc<'a>> {
        nodes.iter().map(|n| self.print_node(n, Args::default())).collect()
    }

    fn print_name_and_args(&self, name: N<'a>, args: &'a [Node<'a>]) -> Doc<'a> {
        let mut doc = vec![self.print_node(name, Args::default())];
        if !args.is_empty() {
            doc.push(",".into());
            doc.push(Doc::Line);
            doc.push(join_with(self.print_all(args), comma_line));
        }
        Doc::Concat(doc)
    }

    #[inline]
    pub fn source_of(&self, node: N<'a>) -> &'a str {
        let p = node.position();
        slice(self.source, p.start as usize, p.end as usize)
    }
}

/// `.replace(/\.\d+$/, '')`.
fn strip_decimal_suffix(value: &str) -> &str {
    if let Some(dot) = value.rfind('.') {
        let digits = &value[dot + 1..];
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            return &value[..dot];
        }
    }
    value
}

/// `text.split(/(\r?\n){2,}/).filter(Boolean)`. The capture group keeps the last newline
/// of each run as its own entry, which is what prints a blank line between paragraphs.
fn paragraphs(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut last = 0;
    let mut i = 0;
    while let Some(off) = memchr::memchr(b'\n', &bytes[i..]) {
        let nl = i + off;
        // A run starts at the first `\r?\n`.
        let run_start = if nl > 0 && bytes[nl - 1] == b'\r' { nl - 1 } else { nl };
        let mut pos = nl + 1;
        let mut count = 1;
        let mut last_nl_start = run_start;
        loop {
            if pos < bytes.len() && bytes[pos] == b'\n' {
                last_nl_start = pos;
                pos += 1;
                count += 1;
            } else if pos + 1 < bytes.len() && bytes[pos] == b'\r' && bytes[pos + 1] == b'\n' {
                last_nl_start = pos;
                pos += 2;
                count += 1;
            } else {
                break;
            }
        }
        if count >= 2 {
            if run_start > last {
                out.push(&text[last..run_start]);
            }
            out.push(&text[last_nl_start..pos]);
            last = pos;
        }
        i = pos;
        if i >= bytes.len() {
            break;
        }
    }
    if last < text.len() {
        out.push(&text[last..]);
    }
    out
}

fn paragraph_fill(paragraph: &str) -> Doc<'_> {
    let trimmed = js_trim(paragraph);
    let mut doc = Vec::new();
    let mut first = true;
    for word in trimmed.split(is_js_ws).filter(|w| !w.is_empty()) {
        if first {
            first = false;
        } else {
            doc.push(Doc::Line);
        }
        doc.push(Doc::Str(word));
    }
    if doc.is_empty() {
        // `''.split(/\s+/)` is `['']`.
        doc.push(Doc::Nil);
    }
    Doc::Fill(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_keep_the_captured_newline() {
        assert_eq!(paragraphs("a\n\nb"), ["a", "\n", "b"]);
        assert_eq!(paragraphs("a\n\n\n\nb"), ["a", "\n", "b"]);
        assert_eq!(paragraphs("a\nb"), ["a\nb"]);
        assert_eq!(paragraphs("a\n \nb"), ["a\n \nb"]);
        assert_eq!(paragraphs("\n\nb"), ["\n", "b"]);
        assert_eq!(paragraphs("a\n\n"), ["a", "\n"]);
    }

    #[test]
    fn number_truncation() {
        assert_eq!(strip_decimal_suffix("1.50"), "1");
        assert_eq!(strip_decimal_suffix("10"), "10");
    }
}
