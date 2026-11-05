//! JSON dump of the AST in upstream's shape, for the parity harness only. UTF-16
//! positions, `source`/`_source` dropped, `undefined` fields omitted.

use crate::ast::*;
use crate::errors::ParseError;
use crate::source_map::SourceMap;

struct W<'m, 's> {
    out: String,
    map: &'m SourceMap<'s>,
}

impl W<'_, '_> {
    fn string(&mut self, s: &str) {
        self.out.push('"');
        let bytes = s.as_bytes();
        let mut run = 0;
        for (i, &b) in bytes.iter().enumerate() {
            if b >= 0x20 && b != b'"' && b != b'\\' {
                continue;
            }
            self.out.push_str(&s[run..i]);
            run = i + 1;
            match b {
                b'"' => self.out.push_str("\\\""),
                b'\\' => self.out.push_str("\\\\"),
                b'\n' => self.out.push_str("\\n"),
                b'\r' => self.out.push_str("\\r"),
                b'\t' => self.out.push_str("\\t"),
                _ => self.out.push_str(&format!("\\u{b:04x}")),
            }
        }
        self.out.push_str(&s[run..]);
        self.out.push('"');
    }

    fn key(&mut self, k: &str) {
        self.out.push(',');
        self.out.push('"');
        self.out.push_str(k);
        self.out.push_str("\":");
    }

    fn pos(&mut self, k: &str, p: Position) {
        self.key(k);
        self.position_value(p);
    }

    fn position_value(&mut self, p: Position) {
        let (a, b) = (self.map.utf16_index(p.start), self.map.utf16_index(p.end));
        self.out.push_str("{\"start\":");
        self.out.push_str(&a.to_string());
        self.out.push_str(",\"end\":");
        self.out.push_str(&b.to_string());
        self.out.push('}');
    }

    fn str_field(&mut self, k: &str, v: &str) {
        self.key(k);
        self.string(v);
    }

    fn opt_str_field(&mut self, k: &str, v: Option<&str>) {
        self.key(k);
        match v {
            Some(s) => self.string(s),
            None => self.out.push_str("null"),
        }
    }

    fn bool_field(&mut self, k: &str, v: bool) {
        self.key(k);
        self.out.push_str(if v { "true" } else { "false" });
    }

    fn list(&mut self, k: &str, nodes: &[Node<'_>]) {
        self.key(k);
        self.array(nodes);
    }

    fn array(&mut self, nodes: &[Node<'_>]) {
        self.out.push('[');
        for (i, n) in nodes.iter().enumerate() {
            if i > 0 {
                self.out.push(',');
            }
            self.node(n);
        }
        self.out.push(']');
    }

    fn child(&mut self, k: &str, n: &Node<'_>) {
        self.key(k);
        self.node(n);
    }

    fn opt_child(&mut self, k: &str, n: &Option<Node<'_>>) {
        self.key(k);
        match n {
            Some(n) => self.node(n),
            None => self.out.push_str("null"),
        }
    }

    fn markup(&mut self, m: &Markup<'_>) {
        self.key("markup");
        self.markup_value(m);
    }

    fn markup_value(&mut self, m: &Markup<'_>) {
        match m {
            Markup::Str(s) => self.string(s),
            Markup::Node(n) => self.node(n),
            Markup::List(l) => self.array(l),
            Markup::None => self.out.push_str("null"),
        }
    }

    fn attr(&mut self, a: &AttrNode<'_>) {
        self.list("name", &a.name);
        self.list("value", &a.value);
        self.pos("attributePosition", a.attribute_position);
    }

    fn node(&mut self, node: &Node<'_>) {
        self.out.push_str("{\"type\":\"");
        self.out.push_str(node.node_type());
        self.out.push('"');
        self.pos("position", node.position());
        match node {
            Node::Document(n) => {
                self.list("children", &n.children);
                self.str_field("name", n.name);
            }
            Node::YamlFrontmatter(n) => self.str_field("body", n.body),
            Node::LiquidRawTag(n) => {
                self.str_field("name", n.name);
                self.str_field("markup", n.markup);
                self.pos("markupPosition", n.markup_position);
                self.child("body", &n.body);
                self.str_field("whitespaceStart", n.whitespace_start);
                self.str_field("whitespaceEnd", n.whitespace_end);
                self.str_field("delimiterWhitespaceStart", n.delimiter_whitespace_start);
                self.str_field("delimiterWhitespaceEnd", n.delimiter_whitespace_end);
                self.pos("blockStartPosition", n.block_start_position);
                self.pos("blockEndPosition", n.block_end_position);
            }
            Node::LiquidTag(n) => {
                self.str_field("name", n.name);
                self.markup(&n.markup);
                if let Some(c) = &n.children {
                    self.list("children", c);
                }
                self.str_field("whitespaceStart", n.whitespace_start);
                self.str_field("whitespaceEnd", n.whitespace_end);
                if let Some(w) = n.delimiter_whitespace_start {
                    self.str_field("delimiterWhitespaceStart", w);
                }
                if let Some(w) = n.delimiter_whitespace_end {
                    self.str_field("delimiterWhitespaceEnd", w);
                }
                self.pos("blockStartPosition", n.block_start_position);
                if let Some(p) = n.block_end_position {
                    self.pos("blockEndPosition", p);
                }
                self.pos("markupPosition", n.markup_position);
                if let Some(r) = &n.reason {
                    self.str_field("reason", r);
                }
            }
            Node::LiquidBranch(n) => {
                self.opt_str_field("name", n.name);
                self.markup(&n.markup);
                self.list("children", &n.children);
                self.str_field("whitespaceStart", n.whitespace_start);
                self.str_field("whitespaceEnd", n.whitespace_end);
                self.pos("markupPosition", n.markup_position);
                self.pos("blockStartPosition", n.block_start_position);
                self.pos("blockEndPosition", n.block_end_position);
            }
            Node::LiquidVariableOutput(n) => {
                self.markup(&n.markup);
                self.str_field("whitespaceStart", n.whitespace_start);
                self.str_field("whitespaceEnd", n.whitespace_end);
                self.pos("markupPosition", n.markup_position);
            }
            Node::HtmlElement(n) => {
                self.list("name", &n.name);
                self.list("attributes", &n.attributes);
                self.list("children", &n.children);
                self.pos("blockStartPosition", n.block_start_position);
                self.pos("blockEndPosition", n.block_end_position);
            }
            Node::HtmlVoidElement(n) => {
                self.str_field("name", n.name);
                self.list("attributes", &n.attributes);
                self.pos("blockStartPosition", n.block_start_position);
            }
            Node::HtmlSelfClosingElement(n) => {
                self.list("name", &n.name);
                self.list("attributes", &n.attributes);
                self.pos("blockStartPosition", n.block_start_position);
            }
            Node::HtmlRawNode(n) => {
                self.str_field("name", n.name);
                self.list("attributes", &n.attributes);
                self.child("body", &n.body);
                self.pos("blockStartPosition", n.block_start_position);
                self.pos("blockEndPosition", n.block_end_position);
            }
            Node::HtmlDoctype(n) => self.opt_str_field("legacyDoctypeString", n.legacy_doctype_string),
            Node::HtmlComment(n) => self.str_field("body", n.body),
            Node::HtmlDanglingMarkerClose(n) => {
                self.list("name", &n.name);
                self.pos("blockStartPosition", n.block_start_position);
            }
            Node::AttrSingleQuoted(a) | Node::AttrDoubleQuoted(a) | Node::AttrUnquoted(a) => {
                self.attr(a)
            }
            Node::AttrEmpty(n) => self.list("name", &n.name),
            Node::TextNode(n) => self.str_field("value", &n.value),
            Node::LiquidVariable(n) => {
                self.child("expression", &n.expression);
                self.list("filters", &n.filters);
                self.str_field("rawSource", n.raw_source);
            }
            Node::LiquidFilter(n) => {
                self.str_field("name", n.name);
                self.list("args", &n.args);
            }
            Node::NamedArgument(n) => {
                self.str_field("name", &n.name);
                self.child("value", &n.value);
            }
            Node::LiquidLiteral(n) => {
                self.str_field("keyword", n.keyword);
                match n.value {
                    LiteralValue::Null => {
                        self.key("value");
                        self.out.push_str("null");
                    }
                    LiteralValue::True => self.bool_field("value", true),
                    LiteralValue::False => self.bool_field("value", false),
                    LiteralValue::EmptyString => self.str_field("value", ""),
                    LiteralValue::ObjectPrototype => {
                        self.key("value");
                        self.out.push_str("{}");
                    }
                    LiteralValue::ObjectPrototypeFunction => {}
                }
            }
            Node::BooleanExpression(n) => self.child("condition", &n.condition),
            Node::String(n) => {
                if let Some(single) = n.single {
                    self.bool_field("single", single);
                }
                self.str_field("value", n.value);
            }
            Node::Number(n) => self.str_field("value", n.value),
            Node::Range(n) => {
                self.child("start", &n.start);
                self.child("end", &n.end);
            }
            Node::VariableLookup(n) => {
                self.opt_str_field("name", n.name);
                self.list("lookups", &n.lookups);
                if let Some(modes) = &n.lookup_modes {
                    self.key("lookupModes");
                    self.out.push('[');
                    for (i, m) in modes.iter().enumerate() {
                        if i > 0 {
                            self.out.push(',');
                        }
                        self.string(m.as_str());
                    }
                    self.out.push(']');
                }
            }
            Node::Comparison(n) => {
                self.str_field("comparator", n.comparator);
                self.child("left", &n.left);
                self.child("right", &n.right);
            }
            Node::LogicalExpression(n) => {
                self.str_field("relation", n.relation.as_str());
                self.child("left", &n.left);
                self.child("right", &n.right);
            }
            Node::AssignMarkup(n) => {
                self.str_field("name", n.name);
                self.child("value", &n.value);
            }
            Node::BlockMarkup(n) => {
                self.child("name", &n.name);
                self.list("args", &n.args);
            }
            Node::ContentForMarkup(n) => {
                self.child("contentForType", &n.content_for_type);
                self.list("args", &n.args);
            }
            Node::CycleMarkup(n) => {
                self.opt_child("groupName", &n.group_name);
                self.list("args", &n.args);
            }
            Node::ForMarkup(n) => {
                self.str_field("variableName", n.variable_name);
                self.child("collection", &n.collection);
                self.bool_field("reversed", n.reversed);
                self.list("args", &n.args);
            }
            Node::PaginateMarkup(n) => {
                self.child("collection", &n.collection);
                self.child("pageSize", &n.page_size);
                self.list("args", &n.args);
            }
            Node::RawMarkup(n) => {
                self.str_field("kind", n.kind.as_str());
                self.str_field("value", n.value);
                self.list("nodes", &n.nodes);
            }
            Node::RenderMarkup(n) => {
                self.child("snippet", &n.snippet);
                self.opt_child("variable", &n.variable);
                self.opt_child("alias", &n.alias);
                self.list("args", &n.args);
            }
            Node::SectionMarkup(n) => {
                self.child("name", &n.name);
                self.list("args", &n.args);
            }
            Node::RenderVariableExpression(n) => {
                self.str_field("kind", n.kind.as_str());
                self.child("name", &n.name);
            }
            Node::RenderAliasExpression(n) => self.str_field("value", n.value),
            Node::LiquidDocParamNode(n) => {
                self.str_field("name", n.name);
                self.child("paramName", &n.param_name);
                self.opt_child("paramType", &n.param_type);
                self.opt_child("paramDescription", &n.param_description);
                self.bool_field("required", n.required);
            }
            Node::LiquidDocDescriptionNode(n) => {
                self.str_field("name", n.name);
                self.child("content", &n.content);
                self.bool_field("isImplicit", n.is_implicit);
                self.bool_field("isInline", n.is_inline);
            }
            Node::LiquidDocExampleNode(n) => {
                self.str_field("name", n.name);
                self.child("content", &n.content);
                self.bool_field("isInline", n.is_inline);
            }
            Node::LiquidDocPromptNode(n) => {
                self.str_field("name", n.name);
                self.child("content", &n.content);
            }
            Node::LiquidErrorNode(n) => {
                self.str_field("message", &n.message);
                if let Some(found) = n.found {
                    self.str_field("found", found);
                }
            }
            Node::BlockArrayLiteral(n) => self.list("elements", &n.elements),
        }
        self.out.push('}');
    }
}

fn writer<'m, 's>(map: &'m SourceMap<'s>) -> W<'m, 's> {
    W { out: String::new(), map }
}

/// `{"ast": <node>}`.
pub fn ast_line(doc: &Document<'_>, map: &SourceMap<'_>) -> String {
    let mut w = writer(map);
    w.out.reserve(doc.source.len() * 4);
    w.out.push_str("{\"ast\":");
    w.node(&doc.root);
    w.out.push('}');
    w.out
}

/// `{"error": {"name", "message", "loc"}}`.
pub fn error_line(e: &ParseError) -> String {
    let source_map = SourceMap::new("");
    let mut w = writer(&source_map);
    w.out.push_str("{\"error\":{\"name\":");
    w.string(&e.name);
    w.out.push_str(",\"message\":");
    w.string(&e.message);
    w.out.push_str(",\"loc\":");
    match &e.loc {
        Some(l) => w.out.push_str(&format!(
            "{{\"start\":{{\"line\":{},\"column\":{}}},\"end\":{{\"line\":{},\"column\":{}}}}}",
            l.start.line, l.start.column, l.end.line, l.end.column
        )),
        None => w.out.push_str("null"),
    }
    w.out.push_str("}}");
    w.out
}

/// A bare node, for lax recovery parity.
pub fn node_json(node: &Node<'_>, map: &SourceMap<'_>) -> String {
    let mut w = writer(map);
    w.node(node);
    w.out
}

/// A `Markup` value, for lax recovery parity.
pub fn markup_json(markup: &Markup<'_>, map: &SourceMap<'_>) -> String {
    let mut w = writer(map);
    w.markup_value(markup);
    w.out
}

/// `{"walk": [[type, start, end], ...], "idsDense": bool}`: the nodes in `walk`
/// order, and whether ids cover `0..node_count` exactly once, for the harness.
pub fn walk_line(doc: &Document<'_>, map: &SourceMap<'_>) -> String {
    fn mark_ids(node: &Node<'_>, seen: &mut [bool], ok: &mut bool) {
        match seen.get_mut(node.id().0 as usize) {
            Some(slot) if !*slot => *slot = true,
            _ => *ok = false,
        }
        // `for_each_child` skips `BlockArrayLiteral` values, so go through the field.
        if let Node::NamedArgument(a) = node {
            mark_ids(&a.value, seen, ok);
        } else {
            node.for_each_child(|c| mark_ids(c, seen, ok));
        }
    }
    let mut seen = vec![false; doc.node_count as usize];
    let mut dense = true;
    mark_ids(&doc.root, &mut seen, &mut dense);
    dense = dense && seen.iter().all(|&s| s);

    let mut w = writer(map);
    w.out.push_str("{\"walk\":[");
    let mut first = true;
    let mut items = Vec::new();
    walk(&doc.root, &mut |n: &Node<'_>, _| items.push((n.node_type(), n.position())));
    for (ty, p) in items {
        if !first {
            w.out.push(',');
        }
        first = false;
        w.out.push_str(&format!(
            "[\"{ty}\",{},{}]",
            map.utf16_index(p.start),
            map.utf16_index(p.end)
        ));
    }
    w.out.push_str(&format!("],\"idsDense\":{dense}}}"));
    w.out
}
