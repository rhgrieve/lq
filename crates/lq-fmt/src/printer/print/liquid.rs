//! Port of `printer/print/liquid.ts`.

use crate::concat;
use crate::doc::utils::{remove_lines, replace_end_of_line};
use crate::doc::{group, group_with, if_break, indent, join_hardline, join_with, Doc};
use crate::printer::string::{
    has_line_break_in_range, is_js_ws, js_trim, js_trim_end, markup_lines, reindent, slice,
    utf16_len,
};
use crate::printer::utils::*;
use crate::printer::*;
use lq_syntax::{LiquidBranch, LiquidTag, Markup, Node, Position, TAGS_WITHOUT_MARKUP};

const LIQUID_TAGS_THAT_ALWAYS_BREAK: [&str; 2] = ["for", "case"];

/// `{ wrapper, prefix, suffix }` of `printNamedLiquidBlockStart`.
struct NamedStart<'a, 'b> {
    is_liquid_statement: bool,
    whitespace_start: &'b mut Option<Doc<'a>>,
    whitespace_end: &'b mut Option<Doc<'a>>,
}

impl<'a> NamedStart<'a, '_> {
    fn wrapper(&self, parts: Vec<Doc<'a>>) -> Doc<'a> {
        if self.is_liquid_statement {
            remove_lines(Doc::Concat(parts))
        } else {
            group(parts)
        }
    }

    fn prefix(&mut self, parts: &mut Vec<Doc<'a>>) {
        if !self.is_liquid_statement {
            parts.push("{%".into());
            parts.push(self.whitespace_start.take().unwrap_or(Doc::Nil));
            parts.push(" ".into());
        }
    }

    fn suffix(&mut self, parts: &mut Vec<Doc<'a>>, trailing_whitespace: Doc<'a>) {
        if !self.is_liquid_statement {
            parts.push(trailing_whitespace);
            parts.push(self.whitespace_end.take().unwrap_or(Doc::Nil));
            parts.push("%}".into());
        }
    }
}

fn markup_of<'a>(node: N<'a>) -> Option<&'a Markup<'a>> {
    match node {
        Node::LiquidTag(t) => Some(&t.markup),
        Node::LiquidBranch(b) => Some(&b.markup),
        _ => None,
    }
}

fn line_or_space<'a>(cond: bool) -> Doc<'a> {
    if cond {
        Doc::Line
    } else {
        " ".into()
    }
}

/// `.replace(/ |\t/g, '').length >= 2` over the source between a last child and a branch.
fn should_add_trailing_newline(source: &str, start: u32, end: u32) -> bool {
    let between = slice(source, start as usize, end as usize);
    if between.is_ascii() {
        return between.bytes().filter(|b| *b != b' ' && *b != b'\t').count() >= 2;
    }
    utf16_len(&between.replace([' ', '\t'], "")) >= 2
}

impl<'a> Ctx<'a> {
    /// `printLiquidVariableOutput`.
    pub fn print_liquid_variable_output(&self, node: N<'a>, args: Args) -> Doc<'a> {
        let Node::LiquidVariableOutput(output) = node else {
            return Doc::Nil;
        };
        let whitespace_start = get_whitespace_trim(
            output.whitespace_start,
            self.has_meaningful_lack_of_leading_whitespace(node),
            args.leading_space_group_id,
        );
        let whitespace_end = get_whitespace_trim(
            output.whitespace_end,
            self.has_meaningful_lack_of_trailing_whitespace(node),
            args.trailing_space_group_id,
        );

        match &output.markup {
            Markup::Str(markup) => {
                let lines = markup_lines(markup);
                if lines.len() > 1 {
                    return group(concat![
                        "{{",
                        whitespace_start,
                        indent(concat![
                            Doc::Hardline,
                            join_hardline(&lines.iter().map(|l| js_trim(l)).collect::<Vec<_>>()),
                        ]),
                        Doc::Hardline,
                        whitespace_end,
                        "}}",
                    ]);
                }
                group(concat!["{{", whitespace_start, " ", *markup, " ", whitespace_end, "}}"])
            }
            Markup::Node(markup) => {
                let has_filters = matches!(&**markup, Node::LiquidVariable(v) if !v.filters.is_empty());
                group(concat![
                    "{{",
                    whitespace_start,
                    indent(concat![
                        line_or_space(has_filters),
                        self.print_node(markup, Args::default())
                    ]),
                    line_or_space(has_filters),
                    whitespace_end,
                    "}}",
                ])
            }
            _ => Doc::Nil,
        }
    }

    fn print_markup_list(&self, list: &'a [Node<'a>], args: Args) -> Doc<'a> {
        join_with(list.iter().map(|p| self.print_node(p, args)), || concat![",", Doc::Line])
    }

    fn print_named_liquid_block_start(
        &self,
        node: N<'a>,
        name: &'a str,
        args: Args,
        whitespace_start: Doc<'a>,
        whitespace_end: Doc<'a>,
    ) -> Doc<'a> {
        let mut ws_start = Some(whitespace_start);
        let mut ws_end = Some(whitespace_end);
        let mut named = NamedStart {
            is_liquid_statement: args.is_liquid_statement,
            whitespace_start: &mut ws_start,
            whitespace_end: &mut ws_end,
        };
        let markup = markup_of(node);

        let tag = |named: &mut NamedStart<'a, '_>, trailing_whitespace: Doc<'a>| -> Doc<'a> {
            let mut parts = Vec::with_capacity(8);
            named.prefix(&mut parts);
            parts.push(name.into());
            parts.push(" ".into());
            if let Some(Markup::Node(m)) = markup {
                parts.push(indent(self.print_node(m, args)));
            }
            named.suffix(&mut parts, trailing_whitespace);
            named.wrapper(parts)
        };

        let tag_with_array_markup =
            |named: &mut NamedStart<'a, '_>, whitespace: Doc<'a>| -> Doc<'a> {
                let mut parts = Vec::with_capacity(8);
                named.prefix(&mut parts);
                parts.push(name.into());
                parts.push(" ".into());
                if let Some(Markup::List(l)) = markup {
                    parts.push(indent(self.print_markup_list(l, args)));
                }
                named.suffix(&mut parts, whitespace);
                named.wrapper(parts)
            };

        let markup_node = match markup {
            Some(Markup::Node(m)) => Some(&**m),
            _ => None,
        };
        let markup_list_len = match markup {
            Some(Markup::List(l)) => l.len(),
            _ => 0,
        };

        match name {
            "echo" => {
                let has = matches!(markup_node, Some(Node::LiquidVariable(v)) if !v.filters.is_empty());
                tag(&mut named, line_or_space(has))
            }
            "assign" => {
                let has = matches!(markup_node, Some(Node::AssignMarkup(a))
                    if matches!(&a.value, Node::LiquidVariable(v) if !v.filters.is_empty()));
                tag(&mut named, line_or_space(has))
            }
            "cycle" => {
                let Some(Node::CycleMarkup(cycle)) = markup_node else {
                    return self.unexpected(name);
                };
                let whitespace = line_or_space(cycle.args.len() > 1);
                let mut parts = Vec::with_capacity(8);
                named.prefix(&mut parts);
                parts.push(name.into());
                parts.push(if cycle.group_name.is_some() { " ".into() } else { Doc::Nil });
                parts.push(indent(self.print_node(markup_node.unwrap(), args)));
                named.suffix(&mut parts, whitespace);
                named.wrapper(parts)
            }
            "content_for" => {
                let has = matches!(markup_node, Some(Node::ContentForMarkup(m)) if !m.args.is_empty());
                tag(&mut named, line_or_space(has))
            }
            "include" | "render" => {
                let has = matches!(markup_node, Some(Node::RenderMarkup(m))
                    if !m.args.is_empty() || (m.variable.is_some() && m.alias.is_some()));
                tag(&mut named, line_or_space(has))
            }
            "capture" | "increment" | "decrement" | "layout" | "section" | "partial" | "sections"
            | "case" => tag(&mut named, " ".into()),
            "block" => {
                let has = matches!(markup_node, Some(Node::BlockMarkup(m)) if !m.args.is_empty());
                tag(&mut named, line_or_space(has))
            }
            "ifchanged" => {
                let mut parts = Vec::with_capacity(6);
                named.prefix(&mut parts);
                parts.push(name.into());
                named.suffix(&mut parts, " ".into());
                named.wrapper(parts)
            }
            "form" | "when" => tag_with_array_markup(&mut named, line_or_space(markup_list_len > 1)),
            "tablerow" | "for" => {
                let has = matches!(markup_node, Some(Node::ForMarkup(m))
                    if m.reversed || !m.args.is_empty());
                tag(&mut named, line_or_space(has))
            }
            "paginate" => tag(&mut named, Doc::Line),
            "if" | "elsif" | "unless" => {
                let has = matches!(
                    markup_node,
                    Some(Node::Comparison(_) | Node::LogicalExpression(_))
                );
                tag(&mut named, line_or_space(has))
            }
            "liquid" => {
                let Some(Markup::List(list)) = markup else {
                    return self.unexpected(name);
                };
                let statements = join_with(
                    list.iter().map(|curr| {
                        concat![
                            self.get_space_between_lines(self.prev(curr), curr),
                            self.print_node(curr, Args { is_liquid_statement: true, ..args }),
                        ]
                    }),
                    || Doc::Hardline,
                );
                let mut parts = Vec::with_capacity(8);
                named.prefix(&mut parts);
                parts.push(name.into());
                parts.push(indent(concat![Doc::Hardline, statements]));
                named.suffix(&mut parts, Doc::Hardline);
                group(parts)
            }
            _ => self.unexpected(name),
        }
    }

    fn unexpected(&self, _name: &str) -> Doc<'a> {
        self.fail("Unexpected object: undefined".to_string());
        Doc::Nil
    }

    fn print_liquid_statement(&self, name: &'a str, markup: &'a str) -> Doc<'a> {
        let should_skip_leading_space =
            js_trim(markup).is_empty() || (name == "#" && markup.starts_with('#'));
        concat![name, if should_skip_leading_space { "" } else { " " }, markup]
    }

    /// `printLiquidBlockStart`.
    pub fn print_liquid_block_start(&self, node: N<'a>, args: Args) -> Doc<'a> {
        let (name, whitespace_start, whitespace_end) = match node {
            Node::LiquidTag(t) => (Some(t.name), t.whitespace_start, t.whitespace_end),
            Node::LiquidBranch(b) => (b.name, b.whitespace_start, b.whitespace_end),
            _ => return Doc::Nil,
        };
        let Some(name) = name.filter(|n| !n.is_empty()) else {
            return Doc::Nil;
        };

        let whitespace_start_doc = get_whitespace_trim(
            whitespace_start,
            self.needs_block_start_leading_whitespace_stripping_on_break(node),
            args.leading_space_group_id,
        );
        let whitespace_end_doc = get_whitespace_trim(
            whitespace_end,
            self.needs_block_start_trailing_whitespace_stripping_on_break(node),
            args.trailing_space_group_id,
        );

        let markup = match markup_of(node) {
            Some(Markup::Str(m)) => *m,
            _ => {
                return self.print_named_liquid_block_start(
                    node,
                    name,
                    args,
                    whitespace_start_doc,
                    whitespace_end_doc,
                );
            }
        };

        if args.is_liquid_statement {
            return self.print_liquid_statement(name, markup);
        }

        let lines = markup_lines(markup);

        if name == "liquid" {
            return group(concat![
                "{%",
                whitespace_start_doc,
                " ",
                name,
                indent(concat![Doc::Hardline, join_hardline(&reindent(&lines, true))]),
                Doc::Hardline,
                whitespace_end_doc,
                "%}",
            ]);
        }

        if lines.len() > 1 {
            return group(concat![
                "{%",
                whitespace_start_doc,
                indent(concat![
                    Doc::Hardline,
                    name,
                    " ",
                    join_hardline(&lines.iter().map(|l| js_trim(l)).collect::<Vec<_>>()),
                ]),
                Doc::Hardline,
                whitespace_end_doc,
                "%}",
            ]);
        }

        let prints_markup = !markup.is_empty() && !TAGS_WITHOUT_MARKUP.contains(&name);
        group(concat![
            "{%",
            whitespace_start_doc,
            " ",
            name,
            if prints_markup { " " } else { "" },
            if prints_markup { markup } else { "" },
            " ",
            whitespace_end_doc,
            "%}",
        ])
    }

    /// `printLiquidBlockEnd`.
    pub fn print_liquid_block_end(&self, node: N<'a>, args: Args) -> Doc<'a> {
        let Node::LiquidTag(tag) = node else {
            return Doc::Nil;
        };
        if tag.children.is_none() || tag.block_end_position.is_none() {
            return Doc::Nil;
        }
        if args.is_liquid_statement {
            return concat!["end", tag.name];
        }
        let whitespace_start = get_whitespace_trim(
            tag.delimiter_whitespace_start.unwrap_or(""),
            self.needs_block_end_leading_whitespace_stripping_on_break(node),
            args.leading_space_group_id,
        );
        let whitespace_end = get_whitespace_trim(
            tag.delimiter_whitespace_end.unwrap_or(""),
            self.has_meaningful_lack_of_trailing_whitespace(node),
            args.trailing_space_group_id,
        );
        group(concat!["{%", whitespace_start, " end", tag.name, " ", whitespace_end, "%}"])
    }

    fn get_liquid_node_content(&self, tag: &LiquidTag<'a>) -> &'a str {
        match (&tag.children, tag.block_end_position) {
            (Some(_), Some(end)) => {
                slice(self.source, tag.block_start_position.end as usize, end.start as usize)
            }
            _ => "",
        }
    }

    /// `printLiquidTag`.
    pub fn print_liquid_tag(&self, node: N<'a>, args: Args) -> Doc<'a> {
        let Node::LiquidTag(tag) = node else {
            return Doc::Nil;
        };
        let (Some(children), Some(_)) = (&tag.children, tag.block_end_position) else {
            return self.print_liquid_block_start(node, args);
        };
        let leading_space_group_id = args.leading_space_group_id;
        let trailing_space_group_id = args.trailing_space_group_id;

        if !args.is_liquid_statement && self.should_preserve_content(node) {
            return concat![
                self.print_liquid_block_start(
                    node,
                    Args {
                        trailing_space_group_id: GroupIds::one(FORCE_FLAT_GROUP_ID),
                        ..args
                    },
                ),
                replace_end_of_line(self.get_liquid_node_content(tag)),
                self.print_liquid_block_end(
                    node,
                    Args { leading_space_group_id: GroupIds::one(FORCE_FLAT_GROUP_ID), ..args },
                ),
            ];
        }

        let tag_group_id = self.symbol();
        let inner_whitespace_group_id = if children.is_empty() {
            GroupIds::one(FORCE_FLAT_GROUP_ID)
        } else {
            GroupIds::one(tag_group_id)
        };
        let block_start = self.print_liquid_block_start(
            node,
            Args {
                leading_space_group_id,
                trailing_space_group_id: inner_whitespace_group_id,
                ..args
            },
        );
        let block_end = self.print_liquid_block_end(
            node,
            Args {
                leading_space_group_id: inner_whitespace_group_id,
                trailing_space_group_id,
                ..args
            },
        );

        let mut body = Doc::Nil;
        if node.is_branched_tag() || tag.name == "tablerow" {
            let branches: Vec<Doc<'a>> = children
                .iter()
                .map(|p| {
                    self.print_node(
                        p,
                        Args {
                            leading_space_group_id: GroupIds::one(tag_group_id),
                            trailing_space_group_id: GroupIds::one(tag_group_id),
                            ..args
                        },
                    )
                })
                .collect();
            body = Doc::Concat(branches);
            if tag.name == "case" {
                body = indent(body);
            }
        } else if !children.is_empty() {
            body = indent(concat![
                self.inner_leading_whitespace(node),
                self.print_children(
                    node,
                    Args {
                        leading_space_group_id: GroupIds::one(tag_group_id),
                        trailing_space_group_id: GroupIds::one(tag_group_id),
                        ..args
                    },
                ),
            ]);
        }

        let p = node.position();
        let should_break = LIQUID_TAGS_THAT_ALWAYS_BREAK.contains(&tag.name)
            || has_line_break_in_range(self.source, p.start as usize, p.end as usize)
            || self.is_attribute_node(node)
            || is_deeply_nested(node);

        group_with(
            concat![block_start, body, self.inner_trailing_whitespace(node, args), block_end],
            tag_group_id,
            should_break,
        )
    }

    /// `printLiquidRawTag`.
    pub fn print_liquid_raw_tag(&self, node: N<'a>, args: Args) -> Doc<'a> {
        let Node::LiquidRawTag(raw) = node else {
            return Doc::Nil;
        };
        let (body_value, body_position) = match &raw.body {
            Node::RawMarkup(b) => (b.value, b.position),
            _ => ("", Position::default()),
        };
        let has_empty_body = js_trim(body_value).is_empty();
        let should_print_as_is = self.has_flag(node, IS_INDENTATION_SENSITIVE)
            || !has_line_break_in_range(
                self.source,
                body_position.start as usize,
                body_position.end as usize,
            );
        let block_start = if args.is_liquid_statement {
            raw.name.into()
        } else {
            let has_markup = !raw.markup.is_empty() && !TAGS_WITHOUT_MARKUP.contains(&raw.name);
            group(concat![
                "{%",
                raw.whitespace_start,
                " ",
                raw.name,
                " ",
                if has_markup { raw.markup } else { "" },
                if has_markup { " " } else { "" },
                raw.whitespace_end,
                "%}",
            ])
        };
        let block_end = if args.is_liquid_statement {
            concat!["end", raw.name]
        } else {
            concat!["{%", raw.whitespace_start, " end", raw.name, " ", raw.whitespace_end, "%}"]
        };

        let body = if should_print_as_is {
            Doc::from(slice(
                self.source,
                raw.block_start_position.end as usize,
                raw.block_end_position.start as usize,
            ))
        } else if has_empty_body {
            Doc::Hardline
        } else {
            self.print_node(&raw.body, Args::default())
        };

        concat![block_start, body, block_end]
    }

    /// `printLiquidDoc`.
    pub fn print_liquid_doc(&self, nodes: &'a [Node<'a>]) -> Doc<'a> {
        if nodes.is_empty() {
            return Doc::Nil;
        }
        let tag_of = |n: N<'a>| -> &'static str {
            match n {
                Node::LiquidDocParamNode(_) => "@param",
                Node::LiquidDocExampleNode(_) => "@example",
                Node::LiquidDocPromptNode(_) => "@prompt",
                Node::LiquidDocDescriptionNode(d) if !d.is_implicit => "@description",
                // `nodes[i][0]` of an implicit description is its text; it never equals a tag.
                _ => "",
            }
        };
        let mut lines = Vec::with_capacity(nodes.len() * 3);
        lines.push(self.print_node(&nodes[0], Args::default()));
        for i in 1..nodes.len() {
            lines.push(Doc::Hardline);
            let (a, b) = (tag_of(&nodes[i - 1]), tag_of(&nodes[i]));
            let same = if a.is_empty() && b.is_empty() {
                self.implicit_description_text(&nodes[i - 1])
                    == self.implicit_description_text(&nodes[i])
            } else {
                a == b
            };
            if !same {
                lines.push(Doc::Hardline);
            }
            lines.push(self.print_node(&nodes[i], Args::default()));
        }
        concat![indent(concat![Doc::Hardline, Doc::Concat(lines)]), Doc::Hardline]
    }

    fn implicit_description_text(&self, n: N<'a>) -> &'a str {
        match n {
            Node::LiquidDocDescriptionNode(d) => js_trim(text_value(&d.content)),
            _ => "",
        }
    }

    /// `printLiquidDocParam`.
    pub fn print_liquid_doc_param(&self, node: N<'a>) -> Doc<'a> {
        let Node::LiquidDocParamNode(param) = node else {
            return Doc::Nil;
        };
        let param_name = text_value(&param.param_name);
        if param_name.is_empty() {
            let p = node.position();
            let raw = slice(self.source, p.start as usize, p.end as usize);
            return concat!["@param", slice(raw, "@param".len(), raw.len())];
        }

        let mut parts: Vec<Doc<'a>> = vec!["@param".into()];

        if let Some(t) = param.param_type.as_ref().map(text_value).filter(|v| !v.is_empty()) {
            parts.push(" ".into());
            parts.push(format!("{{{}}}", js_trim(t)).into());
        }

        parts.push(" ".into());
        if param.required {
            parts.push(js_trim(param_name).into());
        } else {
            parts.push(format!("[{}]", js_trim(param_name)).into());
        }

        if let Some(d) = param.param_description.as_ref().map(text_value).filter(|v| !v.is_empty()) {
            let mut normalized = String::with_capacity(d.len());
            let mut in_ws = false;
            for c in d.chars() {
                if is_js_ws(c) {
                    if !in_ws {
                        normalized.push(' ');
                    }
                    in_ws = true;
                } else {
                    normalized.push(c);
                    in_ws = false;
                }
            }
            let normalized = js_trim(&normalized).to_string();
            parts.push(if self.options.liquid_doc_param_dash { " - " } else { " " }.into());
            parts.push(normalized.into());
        }

        Doc::Concat(parts)
    }

    /// `printLiquidDocExample` and the explicit form of `printLiquidDocDescription`.
    fn print_liquid_doc_block(&self, name: &'a str, content: &'a str, is_inline: bool) -> Doc<'a> {
        concat![
            name,
            if js_trim_end(content).contains('\n') || !is_inline {
                Doc::Hardline
            } else {
                " ".into()
            },
            js_trim(content),
        ]
    }

    pub fn print_liquid_doc_example(&self, node: N<'a>) -> Doc<'a> {
        let Node::LiquidDocExampleNode(example) = node else {
            return Doc::Nil;
        };
        self.print_liquid_doc_block("@example", text_value(&example.content), example.is_inline)
    }

    pub fn print_liquid_doc_description(&self, node: N<'a>) -> Doc<'a> {
        let Node::LiquidDocDescriptionNode(d) = node else {
            return Doc::Nil;
        };
        let content = text_value(&d.content);
        if d.is_implicit {
            return js_trim(content).into();
        }
        self.print_liquid_doc_block("@description", content, d.is_inline)
    }

    pub fn print_liquid_doc_prompt(&self, node: N<'a>) -> Doc<'a> {
        let Node::LiquidDocPromptNode(p) = node else {
            return Doc::Nil;
        };
        concat!["@prompt", js_trim_end(text_value(&p.content))]
    }

    fn inner_leading_whitespace(&self, node: N<'a>) -> Doc<'a> {
        let Some(first) = self.first_child(node) else {
            return if self.has_flag(node, IS_DANGLING_WS_SENSITIVE)
                && self.has_flag(node, HAS_DANGLING_WS)
            {
                Doc::Line
            } else {
                Doc::Nil
            };
        };
        if self.has_flag(first, HAS_LEADING_WS) && self.has_flag(first, IS_LEADING_WS_SENSITIVE) {
            return Doc::Line;
        }
        Doc::Softline
    }

    fn inner_trailing_whitespace(&self, node: N<'a>, args: Args) -> Doc<'a> {
        let last = self.last_child(node);
        if (!args.is_liquid_statement && self.should_preserve_content(node))
            || matches!(node, Node::LiquidBranch(_))
            || block_end_position(node).is_none()
            || last.is_none()
        {
            return Doc::Nil;
        }
        let last = last.unwrap();
        if self.has_flag(last, HAS_TRAILING_WS) && self.has_flag(last, IS_TRAILING_WS_SENSITIVE) {
            return Doc::Line;
        }
        Doc::Softline
    }

    fn print_liquid_default_branch(
        &self,
        node: N<'a>,
        branch: &LiquidBranch<'a>,
        args: Args,
    ) -> Doc<'a> {
        let Some(parent) = self.parent(node) else {
            return Doc::Nil;
        };
        let siblings = children_of(parent).map_or(0, <[_]>::len);

        if branch.children.is_empty() && siblings == 1 {
            return Doc::Nil;
        }
        if branch.children.is_empty() && !self.has_flag(node, HAS_DANGLING_WS) {
            return Doc::Nil;
        }
        if self.has_flag(node, HAS_DANGLING_WS) {
            return if_break(Doc::Nil, " ", 0);
        }

        let add_newline = self.branch_needs_trailing_newline(node, branch);
        indent(concat![
            self.inner_leading_whitespace(parent),
            self.print_children(node, args),
            if add_newline { Doc::Literalline } else { Doc::Nil },
        ])
    }

    fn branch_needs_trailing_newline(&self, node: N<'a>, branch: &LiquidBranch<'a>) -> bool {
        match (self.next(node), branch.children.last()) {
            (Some(next), Some(last)) => {
                should_add_trailing_newline(self.source, last.position().end, next.position().start)
            }
            _ => false,
        }
    }

    /// `printLiquidBranch`.
    pub fn print_liquid_branch(&self, node: N<'a>, args: Args) -> Doc<'a> {
        let Node::LiquidBranch(branch) = node else {
            return Doc::Nil;
        };
        if branch.name.is_none_or(str::is_empty) {
            return self.print_liquid_default_branch(node, branch, args);
        }

        let should_collapse_space = self
            .prev(node)
            .and_then(children_of)
            .is_some_and(<[_]>::is_empty);
        let outer_leading_whitespace =
            if self.has_flag(node, HAS_LEADING_WS) && !should_collapse_space {
                Doc::Line
            } else {
                Doc::Softline
            };
        let add_newline = self.branch_needs_trailing_newline(node, branch);

        concat![
            outer_leading_whitespace,
            self.print_liquid_block_start(node, args),
            indent(concat![
                self.inner_leading_whitespace(node),
                self.print_children(node, args),
                if add_newline { Doc::Literalline } else { Doc::Nil },
            ]),
        ]
    }

    fn needs_block_start_leading_whitespace_stripping_on_break(&self, node: N<'a>) -> bool {
        match node {
            Node::LiquidTag(_) => {
                !self.is_attribute_node(node) && self.has_meaningful_lack_of_leading_whitespace(node)
            }
            Node::LiquidBranch(_) => {
                !self.parent(node).is_some_and(|p| self.is_attribute_node(p))
                    && self.has_meaningful_lack_of_leading_whitespace(node)
            }
            _ => false,
        }
    }

    fn needs_block_start_trailing_whitespace_stripping_on_break(&self, node: N<'a>) -> bool {
        match node {
            Node::LiquidTag(tag) => {
                if node.is_branched_tag() {
                    return match self.first_child(node) {
                        Some(f) => self.needs_block_start_leading_whitespace_stripping_on_break(f),
                        None => false,
                    };
                }
                match &tag.children {
                    None => self.has_meaningful_lack_of_trailing_whitespace(node),
                    Some(c) if c.is_empty() => {
                        self.has_meaningful_lack_of_dangling_whitespace(node)
                    }
                    Some(c) => self.has_meaningful_lack_of_leading_whitespace(&c[0]),
                }
            }
            Node::LiquidBranch(_) => {
                if self.parent(node).is_some_and(|p| self.is_attribute_node(p)) {
                    return false;
                }
                match self.first_child(node) {
                    Some(f) => self.has_meaningful_lack_of_leading_whitespace(f),
                    None => self.has_meaningful_lack_of_dangling_whitespace(node),
                }
            }
            _ => false,
        }
    }

    fn needs_block_end_leading_whitespace_stripping_on_break(&self, node: N<'a>) -> bool {
        if self.is_attribute_node(node) {
            return false;
        }
        match self.last_child(node) {
            Some(l) => self.has_meaningful_lack_of_trailing_whitespace(l),
            None => self.has_meaningful_lack_of_dangling_whitespace(node),
        }
    }

    fn get_space_between_lines(&self, prev: Option<N<'a>>, curr: N<'a>) -> Doc<'a> {
        let Some(prev) = prev else {
            return Doc::Nil;
        };
        let between = slice(self.source, prev.position().end as usize, curr.position().start as usize);
        if memchr::memchr_iter(b'\n', between.as_bytes()).count() > 1 {
            Doc::Hardline
        } else {
            Doc::Nil
        }
    }
}

/// `.value` of the `TextNode`s the LiquidDoc parser builds.
pub fn text_value<'a>(node: N<'a>) -> &'a str {
    match node {
        Node::TextNode(t) => &t.value,
        _ => "",
    }
}
