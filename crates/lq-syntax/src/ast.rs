//! The AST. One `Node` enum, each variant boxing a struct whose public fields are
//! the snake_case names of the upstream fields. `Node` itself is two words.
//!
//! All `Position`s are UTF-8 byte offsets. See `SourceMap` for UTF-16.

use std::borrow::Cow;

/// Unique within a document, dense from 0, assigned pre-order once parsing ends.
/// Use it to index `Vec` side tables of length `Document::node_count`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Position {
    pub start: u32,
    pub end: u32,
}

impl Position {
    #[inline]
    pub fn new(start: u32, end: u32) -> Self {
        Position { start, end }
    }
}

/// The dynamically typed `markup` field of tags, branches and outputs.
#[derive(Clone, Debug)]
pub enum Markup<'s> {
    /// A string: unparsed or base-case markup, also `''` on `else` and `break`.
    Str(&'s str),
    Node(Box<Node<'s>>),
    List(Vec<Node<'s>>),
    /// `null` (`ifchanged`).
    None,
}

/// `{% tag %}` whitespace markers are `"-"` or `""`, as upstream.
pub type WhitespaceMarker = &'static str;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawMarkupKind {
    Css,
    Html,
    Javascript,
    Json,
    Markdown,
    Typescript,
    Text,
}

impl RawMarkupKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RawMarkupKind::Css => "css",
            RawMarkupKind::Html => "html",
            RawMarkupKind::Javascript => "javascript",
            RawMarkupKind::Json => "json",
            RawMarkupKind::Markdown => "markdown",
            RawMarkupKind::Typescript => "typescript",
            RawMarkupKind::Text => "text",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupMode {
    Bareword,
    Subscript,
}

impl LookupMode {
    pub fn as_str(self) -> &'static str {
        match self {
            LookupMode::Bareword => "bareword",
            LookupMode::Subscript => "subscript",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogicalRelation {
    And,
    Or,
}

impl LogicalRelation {
    pub fn as_str(self) -> &'static str {
        match self {
            LogicalRelation::And => "and",
            LogicalRelation::Or => "or",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderVariableKind {
    For,
    With,
}

impl RenderVariableKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RenderVariableKind::For => "for",
            RenderVariableKind::With => "with",
        }
    }
}

/// `LiquidLiteralValues[keyword]`. Upstream tests literals with `in`, which also
/// matches `Object.prototype` members, so `{{ toString }}` is a literal there. In
/// JSON a function value is dropped and `__proto__` serializes as `{}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiteralValue {
    Null,
    True,
    False,
    EmptyString,
    /// `constructor`, `toString`, ...: a function in JS.
    ObjectPrototypeFunction,
    /// `__proto__`: `Object.prototype` in JS.
    ObjectPrototype,
}

macro_rules! node_structs {
    ($($(#[$meta:meta])* $name:ident { $($(#[$fmeta:meta])* $field:ident : $ty:ty),* $(,)? })*) => {
        $(
            $(#[$meta])*
            #[derive(Clone, Debug)]
            pub struct $name<'s> {
                pub id: NodeId,
                pub position: Position,
                $($(#[$fmeta])* pub $field: $ty,)*
            }
        )*
    };
}

node_structs! {
    /// The root node.
    DocumentNode { children: Vec<Node<'s>>, name: &'static str, source: &'s str }
    YamlFrontmatter { body: &'s str }
    LiquidRawTag {
        name: &'s str,
        markup: &'s str,
        markup_position: Position,
        /// Always `Node::RawMarkup`.
        body: Node<'s>,
        whitespace_start: WhitespaceMarker,
        whitespace_end: WhitespaceMarker,
        delimiter_whitespace_start: WhitespaceMarker,
        delimiter_whitespace_end: WhitespaceMarker,
        block_start_position: Position,
        block_end_position: Position,
    }
    LiquidTag {
        name: &'s str,
        markup: Markup<'s>,
        children: Option<Vec<Node<'s>>>,
        whitespace_start: WhitespaceMarker,
        whitespace_end: WhitespaceMarker,
        delimiter_whitespace_start: Option<WhitespaceMarker>,
        delimiter_whitespace_end: Option<WhitespaceMarker>,
        block_start_position: Position,
        block_end_position: Option<Position>,
        markup_position: Position,
        reason: Option<Cow<'s, str>>,
    }
    LiquidBranch {
        /// `None` for the first, unnamed branch.
        name: Option<&'s str>,
        markup: Markup<'s>,
        children: Vec<Node<'s>>,
        whitespace_start: WhitespaceMarker,
        whitespace_end: WhitespaceMarker,
        markup_position: Position,
        block_start_position: Position,
        block_end_position: Position,
    }
    LiquidVariableOutput {
        /// `Str` when unparsable, otherwise `Node(LiquidVariable)`.
        markup: Markup<'s>,
        whitespace_start: WhitespaceMarker,
        whitespace_end: WhitespaceMarker,
        markup_position: Position,
    }
    HtmlElement {
        name: Vec<Node<'s>>,
        attributes: Vec<Node<'s>>,
        children: Vec<Node<'s>>,
        block_start_position: Position,
        block_end_position: Position,
    }
    HtmlVoidElement {
        name: &'s str,
        attributes: Vec<Node<'s>>,
        block_start_position: Position,
    }
    HtmlSelfClosingElement {
        name: Vec<Node<'s>>,
        attributes: Vec<Node<'s>>,
        block_start_position: Position,
    }
    HtmlRawNode {
        name: &'s str,
        attributes: Vec<Node<'s>>,
        /// Always `Node::RawMarkup`.
        body: Node<'s>,
        block_start_position: Position,
        block_end_position: Position,
    }
    HtmlDoctype { legacy_doctype_string: Option<&'s str> }
    HtmlComment { body: &'s str }
    HtmlDanglingMarkerClose { name: Vec<Node<'s>>, block_start_position: Position }
    /// `AttrSingleQuoted`, `AttrDoubleQuoted` and `AttrUnquoted`.
    AttrNode { name: Vec<Node<'s>>, value: Vec<Node<'s>>, attribute_position: Position }
    AttrEmpty { name: Vec<Node<'s>> }
    TextNode { value: Cow<'s, str> }
    LiquidVariable {
        expression: Node<'s>,
        filters: Vec<Node<'s>>,
        raw_source: &'s str,
    }
    LiquidFilter { name: &'s str, args: Vec<Node<'s>> }
    NamedArgument {
        name: Cow<'s, str>,
        /// An expression, or `BlockArrayLiteral` on block and section markup.
        value: Node<'s>,
    }
    LiquidLiteral { keyword: &'s str, value: LiteralValue }
    BooleanExpression { condition: Node<'s> }
    LiquidString {
        /// `None` for lookup segments (`a.b`), which upstream builds without it.
        single: Option<bool>,
        value: &'s str,
    }
    LiquidNumber { value: &'s str }
    LiquidRange { start: Node<'s>, end: Node<'s> }
    LiquidVariableLookup {
        name: Option<&'s str>,
        lookups: Vec<Node<'s>>,
        /// `None` only on nodes built by lax recovery.
        lookup_modes: Option<Vec<LookupMode>>,
    }
    Comparison {
        /// A `Comparators` value; lax recovery can carry an unknown operator.
        comparator: &'s str,
        left: Node<'s>,
        right: Node<'s>,
    }
    LogicalExpression { relation: LogicalRelation, left: Node<'s>, right: Node<'s> }
    AssignMarkup { name: &'s str, value: Node<'s> }
    BlockMarkup { name: Node<'s>, args: Vec<Node<'s>> }
    ContentForMarkup { content_for_type: Node<'s>, args: Vec<Node<'s>> }
    CycleMarkup { group_name: Option<Node<'s>>, args: Vec<Node<'s>> }
    ForMarkup {
        variable_name: &'s str,
        collection: Node<'s>,
        reversed: bool,
        args: Vec<Node<'s>>,
    }
    PaginateMarkup { collection: Node<'s>, page_size: Node<'s>, args: Vec<Node<'s>> }
    RawMarkup { kind: RawMarkupKind, value: &'s str, nodes: Vec<Node<'s>> }
    RenderMarkup {
        snippet: Node<'s>,
        variable: Option<Node<'s>>,
        alias: Option<Node<'s>>,
        args: Vec<Node<'s>>,
    }
    SectionMarkup { name: Node<'s>, args: Vec<Node<'s>> }
    RenderVariableExpression { kind: RenderVariableKind, name: Node<'s> }
    RenderAliasExpression { value: &'s str }
    LiquidDocParamNode {
        name: &'static str,
        param_name: Node<'s>,
        param_type: Option<Node<'s>>,
        param_description: Option<Node<'s>>,
        required: bool,
    }
    LiquidDocDescriptionNode {
        name: &'static str,
        content: Node<'s>,
        is_implicit: bool,
        is_inline: bool,
    }
    LiquidDocExampleNode { name: &'static str, content: Node<'s>, is_inline: bool }
    LiquidDocPromptNode { name: &'static str, content: Node<'s> }
    LiquidErrorNode { message: Cow<'s, str>, found: Option<&'static str> }
    BlockArrayLiteral { elements: Vec<Node<'s>> }
}

#[derive(Clone, Debug)]
pub enum Node<'s> {
    Document(Box<DocumentNode<'s>>),
    YamlFrontmatter(Box<YamlFrontmatter<'s>>),
    LiquidRawTag(Box<LiquidRawTag<'s>>),
    LiquidTag(Box<LiquidTag<'s>>),
    LiquidBranch(Box<LiquidBranch<'s>>),
    LiquidVariableOutput(Box<LiquidVariableOutput<'s>>),
    HtmlSelfClosingElement(Box<HtmlSelfClosingElement<'s>>),
    HtmlVoidElement(Box<HtmlVoidElement<'s>>),
    HtmlDoctype(Box<HtmlDoctype<'s>>),
    HtmlComment(Box<HtmlComment<'s>>),
    HtmlElement(Box<HtmlElement<'s>>),
    HtmlDanglingMarkerClose(Box<HtmlDanglingMarkerClose<'s>>),
    HtmlRawNode(Box<HtmlRawNode<'s>>),
    AttrSingleQuoted(Box<AttrNode<'s>>),
    AttrDoubleQuoted(Box<AttrNode<'s>>),
    AttrUnquoted(Box<AttrNode<'s>>),
    AttrEmpty(Box<AttrEmpty<'s>>),
    TextNode(Box<TextNode<'s>>),
    LiquidVariable(Box<LiquidVariable<'s>>),
    LiquidFilter(Box<LiquidFilter<'s>>),
    NamedArgument(Box<NamedArgument<'s>>),
    LiquidLiteral(Box<LiquidLiteral<'s>>),
    BooleanExpression(Box<BooleanExpression<'s>>),
    String(Box<LiquidString<'s>>),
    Number(Box<LiquidNumber<'s>>),
    Range(Box<LiquidRange<'s>>),
    VariableLookup(Box<LiquidVariableLookup<'s>>),
    Comparison(Box<Comparison<'s>>),
    LogicalExpression(Box<LogicalExpression<'s>>),
    AssignMarkup(Box<AssignMarkup<'s>>),
    BlockMarkup(Box<BlockMarkup<'s>>),
    ContentForMarkup(Box<ContentForMarkup<'s>>),
    CycleMarkup(Box<CycleMarkup<'s>>),
    ForMarkup(Box<ForMarkup<'s>>),
    PaginateMarkup(Box<PaginateMarkup<'s>>),
    RawMarkup(Box<RawMarkup<'s>>),
    RenderMarkup(Box<RenderMarkup<'s>>),
    SectionMarkup(Box<SectionMarkup<'s>>),
    RenderVariableExpression(Box<RenderVariableExpression<'s>>),
    RenderAliasExpression(Box<RenderAliasExpression<'s>>),
    LiquidDocParamNode(Box<LiquidDocParamNode<'s>>),
    LiquidDocDescriptionNode(Box<LiquidDocDescriptionNode<'s>>),
    LiquidDocExampleNode(Box<LiquidDocExampleNode<'s>>),
    LiquidDocPromptNode(Box<LiquidDocPromptNode<'s>>),
    LiquidErrorNode(Box<LiquidErrorNode<'s>>),
    BlockArrayLiteral(Box<BlockArrayLiteral<'s>>),
}

macro_rules! each_variant {
    ($self:expr, $n:ident => $e:expr) => {
        match $self {
            Node::Document($n) => $e,
            Node::YamlFrontmatter($n) => $e,
            Node::LiquidRawTag($n) => $e,
            Node::LiquidTag($n) => $e,
            Node::LiquidBranch($n) => $e,
            Node::LiquidVariableOutput($n) => $e,
            Node::HtmlSelfClosingElement($n) => $e,
            Node::HtmlVoidElement($n) => $e,
            Node::HtmlDoctype($n) => $e,
            Node::HtmlComment($n) => $e,
            Node::HtmlElement($n) => $e,
            Node::HtmlDanglingMarkerClose($n) => $e,
            Node::HtmlRawNode($n) => $e,
            Node::AttrSingleQuoted($n) => $e,
            Node::AttrDoubleQuoted($n) => $e,
            Node::AttrUnquoted($n) => $e,
            Node::AttrEmpty($n) => $e,
            Node::TextNode($n) => $e,
            Node::LiquidVariable($n) => $e,
            Node::LiquidFilter($n) => $e,
            Node::NamedArgument($n) => $e,
            Node::LiquidLiteral($n) => $e,
            Node::BooleanExpression($n) => $e,
            Node::String($n) => $e,
            Node::Number($n) => $e,
            Node::Range($n) => $e,
            Node::VariableLookup($n) => $e,
            Node::Comparison($n) => $e,
            Node::LogicalExpression($n) => $e,
            Node::AssignMarkup($n) => $e,
            Node::BlockMarkup($n) => $e,
            Node::ContentForMarkup($n) => $e,
            Node::CycleMarkup($n) => $e,
            Node::ForMarkup($n) => $e,
            Node::PaginateMarkup($n) => $e,
            Node::RawMarkup($n) => $e,
            Node::RenderMarkup($n) => $e,
            Node::SectionMarkup($n) => $e,
            Node::RenderVariableExpression($n) => $e,
            Node::RenderAliasExpression($n) => $e,
            Node::LiquidDocParamNode($n) => $e,
            Node::LiquidDocDescriptionNode($n) => $e,
            Node::LiquidDocExampleNode($n) => $e,
            Node::LiquidDocPromptNode($n) => $e,
            Node::LiquidErrorNode($n) => $e,
            Node::BlockArrayLiteral($n) => $e,
        }
    };
}

/// Shared child traversal. Field order is the key insertion order of upstream's
/// factories, which is the order `walk` sees via `Object.keys`.
macro_rules! visit_children {
    ($self:expr, $f:ident, $($m:tt)?) => {
        match $self {
            Node::Document(n) => { for c in & $($m)? n.children { $f(c); } }
            Node::LiquidRawTag(n) => $f(& $($m)? n.body),
            Node::LiquidTag(n) => {
                match & $($m)? n.markup {
                    Markup::Node(c) => $f(& $($m)? **c),
                    Markup::List(l) => { for c in l { $f(c); } }
                    Markup::Str(_) | Markup::None => {}
                }
                if let Some(children) = & $($m)? n.children { for c in children { $f(c); } }
            }
            Node::LiquidBranch(n) => {
                match & $($m)? n.markup {
                    Markup::Node(c) => $f(& $($m)? **c),
                    Markup::List(l) => { for c in l { $f(c); } }
                    Markup::Str(_) | Markup::None => {}
                }
                for c in & $($m)? n.children { $f(c); }
            }
            Node::LiquidVariableOutput(n) => {
                match & $($m)? n.markup {
                    Markup::Node(c) => $f(& $($m)? **c),
                    Markup::List(l) => { for c in l { $f(c); } }
                    Markup::Str(_) | Markup::None => {}
                }
            }
            Node::HtmlSelfClosingElement(n) => {
                for c in & $($m)? n.name { $f(c); }
                for c in & $($m)? n.attributes { $f(c); }
            }
            Node::HtmlVoidElement(n) => { for c in & $($m)? n.attributes { $f(c); } }
            Node::HtmlElement(n) => {
                for c in & $($m)? n.name { $f(c); }
                for c in & $($m)? n.attributes { $f(c); }
                for c in & $($m)? n.children { $f(c); }
            }
            Node::HtmlDanglingMarkerClose(n) => { for c in & $($m)? n.name { $f(c); } }
            Node::HtmlRawNode(n) => {
                for c in & $($m)? n.attributes { $f(c); }
                $f(& $($m)? n.body);
            }
            Node::AttrSingleQuoted(n) | Node::AttrDoubleQuoted(n) | Node::AttrUnquoted(n) => {
                for c in & $($m)? n.name { $f(c); }
                for c in & $($m)? n.value { $f(c); }
            }
            Node::AttrEmpty(n) => { for c in & $($m)? n.name { $f(c); } }
            Node::LiquidVariable(n) => {
                $f(& $($m)? n.expression);
                for c in & $($m)? n.filters { $f(c); }
            }
            Node::LiquidFilter(n) => { for c in & $($m)? n.args { $f(c); } }
            Node::NamedArgument(n) => $f(& $($m)? n.value),
            Node::BooleanExpression(n) => $f(& $($m)? n.condition),
            Node::Range(n) => {
                $f(& $($m)? n.start);
                $f(& $($m)? n.end);
            }
            Node::VariableLookup(n) => { for c in & $($m)? n.lookups { $f(c); } }
            Node::Comparison(n) => {
                $f(& $($m)? n.left);
                $f(& $($m)? n.right);
            }
            Node::LogicalExpression(n) => {
                $f(& $($m)? n.left);
                $f(& $($m)? n.right);
            }
            Node::AssignMarkup(n) => $f(& $($m)? n.value),
            Node::BlockMarkup(n) => {
                $f(& $($m)? n.name);
                for c in & $($m)? n.args { $f(c); }
            }
            Node::ContentForMarkup(n) => {
                $f(& $($m)? n.content_for_type);
                for c in & $($m)? n.args { $f(c); }
            }
            Node::CycleMarkup(n) => {
                if let Some(c) = & $($m)? n.group_name { $f(c); }
                for c in & $($m)? n.args { $f(c); }
            }
            Node::ForMarkup(n) => {
                $f(& $($m)? n.collection);
                for c in & $($m)? n.args { $f(c); }
            }
            Node::PaginateMarkup(n) => {
                $f(& $($m)? n.collection);
                $f(& $($m)? n.page_size);
                for c in & $($m)? n.args { $f(c); }
            }
            Node::RawMarkup(n) => { for c in & $($m)? n.nodes { $f(c); } }
            Node::RenderMarkup(n) => {
                $f(& $($m)? n.snippet);
                if let Some(c) = & $($m)? n.variable { $f(c); }
                if let Some(c) = & $($m)? n.alias { $f(c); }
                for c in & $($m)? n.args { $f(c); }
            }
            Node::SectionMarkup(n) => {
                $f(& $($m)? n.name);
                for c in & $($m)? n.args { $f(c); }
            }
            Node::RenderVariableExpression(n) => $f(& $($m)? n.name),
            Node::LiquidDocParamNode(n) => {
                $f(& $($m)? n.param_name);
                if let Some(c) = & $($m)? n.param_type { $f(c); }
                if let Some(c) = & $($m)? n.param_description { $f(c); }
            }
            Node::LiquidDocDescriptionNode(n) => $f(& $($m)? n.content),
            Node::LiquidDocExampleNode(n) => $f(& $($m)? n.content),
            Node::LiquidDocPromptNode(n) => $f(& $($m)? n.content),
            Node::BlockArrayLiteral(n) => { for c in & $($m)? n.elements { $f(c); } }
            Node::YamlFrontmatter(_)
            | Node::HtmlDoctype(_)
            | Node::HtmlComment(_)
            | Node::TextNode(_)
            | Node::LiquidLiteral(_)
            | Node::String(_)
            | Node::Number(_)
            | Node::RenderAliasExpression(_)
            | Node::LiquidErrorNode(_) => {}
        }
    };
}

/// The upstream `name` field, whose type varies with the node type.
#[derive(Clone, Copy, Debug)]
pub enum NodeName<'a, 's> {
    Str(&'a str),
    Null,
    Parts(&'a [Node<'s>]),
    Node(&'a Node<'s>),
}

impl<'s> Node<'s> {
    #[inline]
    pub fn id(&self) -> NodeId {
        each_variant!(self, n => n.id)
    }

    #[inline]
    pub fn position(&self) -> Position {
        each_variant!(self, n => n.position)
    }

    /// The upstream `type` string.
    pub fn node_type(&self) -> &'static str {
        match self {
            Node::Document(_) => "Document",
            Node::YamlFrontmatter(_) => "YAMLFrontmatter",
            Node::LiquidRawTag(_) => "LiquidRawTag",
            Node::LiquidTag(_) => "LiquidTag",
            Node::LiquidBranch(_) => "LiquidBranch",
            Node::LiquidVariableOutput(_) => "LiquidVariableOutput",
            Node::HtmlSelfClosingElement(_) => "HtmlSelfClosingElement",
            Node::HtmlVoidElement(_) => "HtmlVoidElement",
            Node::HtmlDoctype(_) => "HtmlDoctype",
            Node::HtmlComment(_) => "HtmlComment",
            Node::HtmlElement(_) => "HtmlElement",
            Node::HtmlDanglingMarkerClose(_) => "HtmlDanglingMarkerClose",
            Node::HtmlRawNode(_) => "HtmlRawNode",
            Node::AttrSingleQuoted(_) => "AttrSingleQuoted",
            Node::AttrDoubleQuoted(_) => "AttrDoubleQuoted",
            Node::AttrUnquoted(_) => "AttrUnquoted",
            Node::AttrEmpty(_) => "AttrEmpty",
            Node::TextNode(_) => "TextNode",
            Node::LiquidVariable(_) => "LiquidVariable",
            Node::LiquidFilter(_) => "LiquidFilter",
            Node::NamedArgument(_) => "NamedArgument",
            Node::LiquidLiteral(_) => "LiquidLiteral",
            Node::BooleanExpression(_) => "BooleanExpression",
            Node::String(_) => "String",
            Node::Number(_) => "Number",
            Node::Range(_) => "Range",
            Node::VariableLookup(_) => "VariableLookup",
            Node::Comparison(_) => "Comparison",
            Node::LogicalExpression(_) => "LogicalExpression",
            Node::AssignMarkup(_) => "AssignMarkup",
            Node::BlockMarkup(_) => "BlockMarkup",
            Node::ContentForMarkup(_) => "ContentForMarkup",
            Node::CycleMarkup(_) => "CycleMarkup",
            Node::ForMarkup(_) => "ForMarkup",
            Node::PaginateMarkup(_) => "PaginateMarkup",
            Node::RawMarkup(_) => "RawMarkup",
            Node::RenderMarkup(_) => "RenderMarkup",
            Node::SectionMarkup(_) => "SectionMarkup",
            Node::RenderVariableExpression(_) => "RenderVariableExpression",
            Node::RenderAliasExpression(_) => "RenderAliasExpression",
            Node::LiquidDocParamNode(_) => "LiquidDocParamNode",
            Node::LiquidDocDescriptionNode(_) => "LiquidDocDescriptionNode",
            Node::LiquidDocExampleNode(_) => "LiquidDocExampleNode",
            Node::LiquidDocPromptNode(_) => "LiquidDocPromptNode",
            Node::LiquidErrorNode(_) => "LiquidErrorNode",
            Node::BlockArrayLiteral(_) => "BlockArrayLiteral",
        }
    }

    /// The upstream `name` field, for node types that have one.
    pub fn name(&self) -> Option<NodeName<'_, 's>> {
        Some(match self {
            Node::Document(n) => NodeName::Str(n.name),
            Node::LiquidRawTag(n) => NodeName::Str(n.name),
            Node::LiquidTag(n) => NodeName::Str(n.name),
            Node::LiquidBranch(n) => n.name.map_or(NodeName::Null, NodeName::Str),
            Node::HtmlElement(n) => NodeName::Parts(&n.name),
            Node::HtmlSelfClosingElement(n) => NodeName::Parts(&n.name),
            Node::HtmlDanglingMarkerClose(n) => NodeName::Parts(&n.name),
            Node::HtmlVoidElement(n) => NodeName::Str(n.name),
            Node::HtmlRawNode(n) => NodeName::Str(n.name),
            Node::AttrSingleQuoted(n) | Node::AttrDoubleQuoted(n) | Node::AttrUnquoted(n) => {
                NodeName::Parts(&n.name)
            }
            Node::AttrEmpty(n) => NodeName::Parts(&n.name),
            Node::LiquidFilter(n) => NodeName::Str(n.name),
            Node::NamedArgument(n) => NodeName::Str(&n.name),
            Node::VariableLookup(n) => n.name.map_or(NodeName::Null, NodeName::Str),
            Node::AssignMarkup(n) => NodeName::Str(n.name),
            Node::BlockMarkup(n) => NodeName::Node(&n.name),
            Node::SectionMarkup(n) => NodeName::Node(&n.name),
            Node::RenderVariableExpression(n) => NodeName::Node(&n.name),
            Node::LiquidDocParamNode(n) => NodeName::Str(n.name),
            Node::LiquidDocDescriptionNode(n) => NodeName::Str(n.name),
            Node::LiquidDocExampleNode(n) => NodeName::Str(n.name),
            Node::LiquidDocPromptNode(n) => NodeName::Str(n.name),
            _ => return None,
        })
    }

    /// Visits direct children in the order of upstream's `walk`.
    ///
    /// As upstream, a `BlockArrayLiteral` that is the value of a `NamedArgument`
    /// is not visited: its `type` is not in `NodeTypes`, so `isLiquidHtmlNode`
    /// rejects it. Reach it through `NamedArgument::value` when needed.
    pub fn for_each_child<'a, F: FnMut(&'a Node<'s>)>(&'a self, mut f: F) {
        if let Node::NamedArgument(n) = self {
            if matches!(n.value, Node::BlockArrayLiteral(_)) {
                return;
            }
        }
        visit_children!(self, f,);
    }

    /// Like `for_each_child`, but includes `BlockArrayLiteral` values, so it
    /// reaches every node in the tree.
    pub fn for_each_child_mut<F: FnMut(&mut Node<'s>)>(&mut self, mut f: F) {
        visit_children!(self, f, mut);
    }

    /// Whether `isBranchedTag(node)`.
    pub fn is_branched_tag(&self) -> bool {
        matches!(self, Node::LiquidTag(t) if matches!(t.name, "if" | "for" | "unless" | "case"))
    }
}

impl<'s> TextNode<'s> {
    pub fn as_str(&self) -> &str {
        &self.value
    }
}

/// Upstream's `walk`: post-order, passing each node and its parent.
pub fn walk<'a, 's, F: FnMut(&'a Node<'s>, Option<&'a Node<'s>>)>(root: &'a Node<'s>, f: &mut F) {
    fn inner<'a, 's, F: FnMut(&'a Node<'s>, Option<&'a Node<'s>>)>(
        node: &'a Node<'s>,
        parent: Option<&'a Node<'s>>,
        f: &mut F,
    ) {
        node.for_each_child(|c| inner(c, Some(node), f));
        f(node, parent);
    }
    inner(root, None, f);
}

/// Assigns dense pre-order ids starting at `next`, returning the next free id.
/// Parsing does this for you; it is for trees from `MarkupParser` and lax recovery.
pub fn assign_node_ids(node: &mut Node<'_>, next: u32) -> u32 {
    fn inner(node: &mut Node<'_>, next: &mut u32) {
        let id = NodeId(*next);
        each_variant!(node, n => n.id = id);
        *next += 1;
        node.for_each_child_mut(|c| inner(c, next));
    }
    let mut next = next;
    inner(node, &mut next);
    next
}

/// A parsed document.
#[derive(Clone, Debug)]
pub struct Document<'s> {
    pub source: &'s str,
    /// Always `Node::Document`.
    pub root: Node<'s>,
    /// The number of nodes; ids are `0..node_count`.
    pub node_count: u32,
}

impl<'s> Document<'s> {
    pub fn children(&self) -> &[Node<'s>] {
        match &self.root {
            Node::Document(d) => &d.children,
            _ => &[],
        }
    }
}
