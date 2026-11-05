//! `DeprecatedFilter`, with `deprecated-filter/fixes.ts`.

use crate::api::*;
use lq_json::js::number_to_string;
use lq_syntax::{LiquidFilter, Node};
use lq_theme::docs::{FilterEntry, ThemeDocset};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidFilter]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    cx.docset()?;
    Some(Box::new(DeprecatedFilter { deprecated: None }))
}

struct DeprecatedFilter {
    /// The names of the deprecated filters, computed when the file first has a filter.
    deprecated: Option<Vec<&'static str>>,
}

/// `reason.match(/replaced by \[`(.+?)`\]/)?.[1]`.
fn recommended_alternative(reason: &str) -> Option<&str> {
    const PREFIX: &str = "replaced by [`";
    for (at, _) in reason.match_indices(PREFIX) {
        let body = &reason[at + PREFIX.len()..];
        // `.+?`: at least one character, none of them a line terminator.
        let first_width = body.chars().next().map_or(0, char::len_utf8);
        let Some(close) = body.get(first_width..).and_then(|rest| rest.find("`]")).map(|i| i + first_width) else {
            continue;
        };
        if first_width > 0 && !body[..close].contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
            return Some(&body[..close]);
        }
    }
    None
}

/// A width or height. `Text` is what `size in NAMED_SIZES` finds on `Object.prototype`.
#[derive(Clone, Copy)]
enum Dim {
    Num(f64),
    Text(&'static str),
}

impl Dim {
    fn is_truthy(self) -> bool {
        match self {
            Dim::Num(n) => n != 0.0 && !n.is_nan(),
            Dim::Text(_) => true,
        }
    }

    fn is_minus_one(self) -> bool {
        matches!(self, Dim::Num(n) if n == -1.0)
    }

    fn scaled(self, scale: f64) -> Dim {
        match self {
            Dim::Num(n) => Dim::Num(n * scale),
            Dim::Text(_) => Dim::Num(f64::NAN),
        }
    }

    fn limited(self) -> Dim {
        match self {
            // `Math.min`: NaN when either is.
            Dim::Num(n) => Dim::Num(if n.is_nan() { n } else { n.min(MAX_SIZE) }),
            Dim::Text(_) => Dim::Num(f64::NAN),
        }
    }
}

type ImageSize = (Dim, Dim);

const MAX_SIZE: f64 = 5760.0;

///
/// Without `own_only` ([`Divergence::ImgUrlPrototype`]) it finds what `size in NAMED_SIZES` does.
fn named_size(size: &str, own_only: bool) -> Option<Dim> {
    Some(match size {
        "pico" => Dim::Num(16.0),
        "icon" => Dim::Num(32.0),
        "thumb" => Dim::Num(50.0),
        "small" => Dim::Num(100.0),
        "compact" => Dim::Num(160.0),
        "medium" => Dim::Num(240.0),
        "large" => Dim::Num(480.0),
        "grande" => Dim::Num(600.0),
        "original" => Dim::Num(1024.0),
        _ if own_only => return None,
        "constructor" => Dim::Text("function Object() { [native code] }"),
        "__defineGetter__" => Dim::Text("function __defineGetter__() { [native code] }"),
        "__defineSetter__" => Dim::Text("function __defineSetter__() { [native code] }"),
        "hasOwnProperty" => Dim::Text("function hasOwnProperty() { [native code] }"),
        "__lookupGetter__" => Dim::Text("function __lookupGetter__() { [native code] }"),
        "__lookupSetter__" => Dim::Text("function __lookupSetter__() { [native code] }"),
        "isPrototypeOf" => Dim::Text("function isPrototypeOf() { [native code] }"),
        "propertyIsEnumerable" => Dim::Text("function propertyIsEnumerable() { [native code] }"),
        "toString" => Dim::Text("function toString() { [native code] }"),
        "valueOf" => Dim::Text("function valueOf() { [native code] }"),
        "toLocaleString" => Dim::Text("function toLocaleString() { [native code] }"),
        "__proto__" => Dim::Text("[object Object]"),
        _ => return None,
    })
}

/// `parseInt(s)`.
fn parse_int(s: &str) -> f64 {
    let t = lq_syntax::js::trim_start(s);
    let (negative, t) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let (radix, t) = match t.get(..2) {
        Some("0x" | "0X") => (16, &t[2..]),
        _ => (10, t),
    };
    let digits = t.chars().map_while(|c| c.to_digit(radix));
    let mut value = None;
    for digit in digits {
        value = Some(value.unwrap_or(0.0) * f64::from(radix) + f64::from(digit));
    }
    match value {
        Some(n) if negative => -n,
        Some(n) => n,
        None => f64::NAN,
    }
}

fn image_size(size: Option<&str>, default: f64, own_only: bool) -> ImageSize {
    let Some(size) = size.filter(|s| !s.is_empty()) else { return (Dim::Num(default), Dim::Num(default)) };
    if let Some(named) = named_size(size, own_only) {
        return (named, named);
    }
    let mut parts = size.split('x').map(parse_int);
    // A missing height is `undefined`, which `Math.min` makes NaN.
    let width = parts.next().unwrap_or(f64::NAN);
    let height = parts.next().unwrap_or(f64::NAN);
    (Dim::Num(width).limited(), Dim::Num(height).limited())
}

fn scale_image(filter: &LiquidFilter<'_>, (width, height): ImageSize) -> ImageSize {
    let scale = named_argument(filter, "scale")
        .and_then(|v| literal_value(v))
        .filter(|v| !v.is_empty())
        .map_or(0.0, parse_int);
    let scale = if scale == 0.0 || scale.is_nan() { 1.0 } else { scale };
    (width.scaled(scale).limited(), height.scaled(scale).limited())
}

fn ensure_image_value((width, height): ImageSize) -> ImageSize {
    let unset = |d: Dim| !d.is_truthy() || d.is_minus_one();
    if unset(height) && unset(width) {
        (Dim::Num(100.0), height)
    } else {
        (width, height)
    }
}

fn literal_value<'n>(node: &'n Node<'_>) -> Option<&'n str> {
    match node {
        Node::Number(n) => Some(n.value),
        Node::String(s) => Some(s.value),
        _ => None,
    }
}

fn expression_argument<'n>(filter: &'n LiquidFilter<'_>, index: usize) -> Option<&'n str> {
    filter.args.get(index).and_then(literal_value)
}

/// The value of a named argument.
fn named_argument<'n, 's>(filter: &'n LiquidFilter<'s>, name: &str) -> Option<&'n Node<'s>> {
    filter.args.iter().find_map(|arg| match arg {
        Node::NamedArgument(a) if a.name == name => Some(&a.value),
        _ => None,
    })
}

/// `buildFilterString`, whose parameter order is the order of `add` calls.
struct FilterString(String, bool);

impl FilterString {
    fn new(filter: &str) -> FilterString {
        FilterString(format!(" {filter}"), false)
    }

    fn add(&mut self, key: &str, value: &str) {
        self.0.push_str(if self.1 { ", " } else { ": " });
        self.1 = true;
        self.0.push_str(key);
        self.0.push_str(": ");
        self.0.push_str(value);
    }

    fn dim(mut self, key: &str, value: Dim) -> Self {
        if value.is_truthy() && !value.is_minus_one() {
            match value {
                Dim::Num(n) => self.add(key, &number_to_string(n)),
                Dim::Text(t) => self.add(key, t),
            }
        }
        self
    }

    /// `strValue(value)`.
    fn quoted(mut self, key: &str, value: Option<&str>) -> Self {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            self.add(key, &format!("'{value}'"));
        }
        self
    }

    fn raw(mut self, key: &str, value: Option<&str>) -> Self {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            self.add(key, value);
        }
        self
    }
}

/// `getFilterSourceStartAndEnd(node)`.
fn filter_span(node: &Node<'_>, source: &str) -> (u32, u32) {
    let p = node.position();
    let start = source.get(p.start as usize..).and_then(|s| s.find('|')).map_or(p.start, |i| p.start + i as u32 + 1);
    (start, p.end)
}

fn img_tag_suggestion(filter: &LiquidFilter<'_>, own_only: bool) -> String {
    let (width, height) = image_size(expression_argument(filter, 2), -1.0, own_only);
    let (url_width, url_height) = ensure_image_value((width, height));
    let url = FilterString::new("image_url").dim("width", url_width).dim("height", url_height);
    let tag = FilterString::new("image_tag")
        .dim("width", width)
        .dim("height", height)
        .quoted("alt", expression_argument(filter, 0))
        .quoted("class", expression_argument(filter, 1));
    format!("{} |{}", url.0, tag.0)
}

/// `None` when `scale` or the size is a variable lookup.
fn img_url_suggestion(filter: &LiquidFilter<'_>, own_only: bool) -> Option<String> {
    let scale = named_argument(filter, "scale");
    if matches!(scale, Some(Node::VariableLookup(_))) || matches!(filter.args.first(), Some(Node::VariableLookup(_))) {
        return None;
    }
    let size = expression_argument(filter, 0);
    let (width, height) = ensure_image_value(scale_image(filter, image_size(size, 100.0, own_only)));
    let mut text = FilterString::new("image_url").dim("width", width).dim("height", height);
    if let Some(Node::String(crop)) = named_argument(filter, "crop") {
        text = text.quoted("crop", Some(crop.value));
    }
    match named_argument(filter, "format") {
        Some(Node::String(format)) => text = text.quoted("format", Some(format.value)),
        Some(Node::VariableLookup(format)) => text = text.raw("format", format.name),
        _ => {}
    }
    Some(text.0)
}

fn image_url_suggestion(filter: &LiquidFilter<'_>, own_only: bool) -> String {
    let (width, height) = ensure_image_value(image_size(expression_argument(filter, 0), 100.0, own_only));
    FilterString::new("image_url").dim("width", width).dim("height", height).0
}

fn hex_to_rgba_fix(filter: &LiquidFilter<'_>) -> Option<String> {
    if filter.args.len() > 1 {
        return None;
    }
    Some(match expression_argument(filter, 0).filter(|a| !a.is_empty()) {
        Some(alpha) => format!(" color_to_rgb | color_modify: 'alpha', {alpha}"),
        None => " color_to_rgb".into(),
    })
}

fn deprecated_entry<'d>(docset: &'d dyn ThemeDocset, name: &str) -> Option<&'d FilterEntry> {
    docset.filters().iter().find(|f| f.deprecated && f.name == name)
}

impl<'a> LiquidCheck<'a> for DeprecatedFilter {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidFilter(filter) = node else { return };
        let Some(docset) = cx.docset() else { return };
        let deprecated = self
            .deprecated
            .get_or_insert_with(|| docset.filters().iter().filter(|f| f.deprecated).map(|f| f.name).collect());
        if !deprecated.contains(&filter.name) {
            return;
        }
        let Some(entry) = deprecated_entry(docset, filter.name) else { return };

        let recommended = entry
            .deprecation_reason
            .and_then(recommended_alternative)
            .and_then(|name| docset.filter(name));
        let message = match recommended {
            Some(r) => format!("Deprecated filter '{}', consider using '{}'.", entry.name, r.name),
            None => format!("Deprecated filter '{}'.", entry.name),
        };

        let p = node.position();
        let mut problem = LiquidProblem::new(message, p.start + 1, p.end);
        let (start, end) = filter_span(node, cx.source());
        let own_only = cx.divergence(Divergence::ImgUrlPrototype);
        let suggestion = match filter.name {
            "img_tag" => Some(("Replace 'img_tag' with 'image_tag'.".to_string(), img_tag_suggestion(filter, own_only))),
            "img_url" => img_url_suggestion(filter, own_only).map(|text| ("Replace 'img_url' with 'image_url'.".to_string(), text)),
            "article_img_url" | "collection_img_url" | "product_img_url" => {
                Some((format!("Replace '{}' with 'image_url'.", filter.name), image_url_suggestion(filter, own_only)))
            }
            _ => None,
        };
        if let Some((message, text)) = suggestion {
            problem = problem.suggest(message, |c| c.replace(start, end, text));
        }
        if filter.name == "hex_to_rgba" {
            if let Some(text) = hex_to_rgba_fix(filter) {
                problem = problem.fix(|c| c.replace(start, end, text));
            }
        }
        cx.report(problem);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_names() {
        assert_eq!(recommended_alternative("replaced by [`image_url`](/x)."), Some("image_url"));
        assert_eq!(recommended_alternative("no"), None);
        assert_eq!(recommended_alternative("replaced by [``] and [`a`]"), Some("`] and [`a"));
        assert_eq!(recommended_alternative("replaced by [`a\nb`]"), None);
    }

    #[test]
    fn parse_int_matches_js() {
        assert_eq!(parse_int("100"), 100.0);
        assert_eq!(parse_int(" 12px"), 12.0);
        assert_eq!(parse_int("-3"), -3.0);
        assert_eq!(parse_int("0x10"), 16.0);
        assert!(parse_int("").is_nan());
        assert!(parse_int("px").is_nan());
        assert!(parse_int("0x").is_nan());
    }
}
