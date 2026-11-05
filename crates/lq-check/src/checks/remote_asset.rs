//! `RemoteAsset`.

use crate::api::*;
use crate::check_utils::{html_attributes, html_node_name, is_attr, valued_attribute};
use lq_syntax::{AttrNode, Markup, Node};
use std::borrow::Cow;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[
        NodeKind::HtmlVoidElement,
        NodeKind::HtmlSelfClosingElement,
        NodeKind::HtmlRawNode,
        NodeKind::LiquidFilter,
    ]),
    exit: NodeMask::NONE,
};

const SHOPIFY_CDN_DOMAINS: [&str; 2] = ["fonts.shopifycdn.com", "cdn.shopify.com"];

const ASSET_URL_FILTERS: [&str; 13] = [
    "product_img_url",
    "article_img_url",
    "collection_img_url",
    "img_url",
    "asset_url",
    "image_url",
    "asset_img_url",
    "file_img_url",
    "file_url",
    "global_asset_url",
    "shopify_asset_url",
    "external_video_url",
    "font_url",
];

const ASSET_MESSAGE: &str = "Use one of the asset_url filters to serve assets for better performance.";
const CDN_MESSAGE: &str = "Asset should be served by the Shopify CDN for better performance.";

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    // `normaliseAllowedDomains`.
    let allowed_domains = cx
        .settings()
        .strs("allowedDomains")
        .into_iter()
        .filter_map(url_hostname)
        .map(|host| host.strip_prefix("www.").map_or_else(|| host.clone(), str::to_string))
        .collect();
    Some(Box::new(RemoteAsset { allowed_domains }))
}

struct RemoteAsset {
    allowed_domains: Vec<String>,
}

/// `url.replace(/[“”‘’]/g, '')`.
fn clean_url_text(url: &str) -> Cow<'_, str> {
    const QUOTES: [char; 4] = ['\u{201c}', '\u{201d}', '\u{2018}', '\u{2019}'];
    if url.contains(QUOTES) {
        Cow::Owned(url.replace(QUOTES, ""))
    } else {
        Cow::Borrowed(url)
    }
}

/// `/^data:/i`.
fn is_data_uri(url: &str) -> bool {
    url.as_bytes().get(..5).is_some_and(|b| b.eq_ignore_ascii_case(b"data:"))
}

/// `/^https?:\/\//`.
fn starts_with_http(url: &str) -> bool {
    let rest = url.strip_prefix("http").unwrap_or("");
    rest.strip_prefix('s').unwrap_or(rest).starts_with("://")
}

/// `/^(https?:)?\/\//`.
fn starts_with_optional_http(url: &str) -> bool {
    url.starts_with("//") || starts_with_http(url)
}

fn is_url_hosted_by_shopify(url: &str, allowed: &[String]) -> bool {
    if url.starts_with("/cdn/") {
        return true;
    }
    let Some(host) = url_hostname(url) else { return false };
    SHOPIFY_CDN_DOMAINS.contains(&host.as_str()) || allowed.contains(&host)
}

fn variable_of<'n, 's>(node: &'n Node<'s>) -> Option<&'n lq_syntax::LiquidVariable<'s>> {
    let Node::LiquidVariableOutput(output) = node else { return None };
    let Markup::Node(markup) = &output.markup else { return None };
    match &**markup {
        Node::LiquidVariable(v) => Some(v),
        _ => None,
    }
}

fn value_is_definitely_not_shopify_hosted(attr: &AttrNode<'_>, allowed: &[String]) -> bool {
    attr.value.iter().any(|node| {
        if let Node::TextNode(text) = node {
            let url = clean_url_text(&text.value);
            if starts_with_optional_http(&url) && !is_url_hosted_by_shopify(&url, allowed) {
                return true;
            }
        }
        let Some(variable) = variable_of(node) else { return false };
        matches!(&variable.expression, Node::String(s)
            if starts_with_http(s.value) && !is_url_hosted_by_shopify(s.value, allowed))
    })
}

fn value_is_shopify_hosted(attr: &AttrNode<'_>) -> bool {
    attr.value.iter().any(|node| {
        let Some(variable) = variable_of(node) else { return false };
        variable.filters.iter().any(|f| matches!(f, Node::LiquidFilter(f) if ASSET_URL_FILTERS.contains(&f.name)))
            || matches!(&variable.expression, Node::VariableLookup(l) if l.name == Some("canonical_url"))
    })
}

impl RemoteAsset {
    fn check_html_node(&self, node: &Node<'_>, cx: &LiquidCx<'_, '_>) {
        if !matches!(html_node_name(node), Some("img" | "link" | "source" | "script")) {
            return;
        }
        let Some(attr) = html_attributes(node)
            .iter()
            .find_map(|a| valued_attribute(a).filter(|_| is_attr(a, "src") || is_attr(a, "href")))
        else {
            return;
        };
        let mut texts = attr.value.iter().filter_map(|n| match n {
            Node::TextNode(t) => Some(&*t.value),
            _ => None,
        });
        if let Some(first) = texts.clone().next() {
            let first = clean_url_text(first);
            if first.starts_with('#') || is_data_uri(&first) {
                return;
            }
        }
        // A value starting with a variable lookup can't be analyzed statically.
        if attr.value.first().and_then(variable_of).is_some_and(|v| matches!(v.expression, Node::VariableLookup(_))) {
            return;
        }
        if texts.any(|t| is_url_hosted_by_shopify(&clean_url_text(t), &self.allowed_domains)) {
            return;
        }
        let p = attr.position;
        if value_is_definitely_not_shopify_hosted(attr, &self.allowed_domains) {
            cx.report(LiquidProblem::new(CDN_MESSAGE, p.start, p.end));
            return;
        }
        if !value_is_shopify_hosted(attr) {
            cx.report(LiquidProblem::new(ASSET_MESSAGE, p.start, p.end));
        }
    }

    fn check_filter(&self, node: &Node<'_>, ancestors: &[&Node<'_>], cx: &LiquidCx<'_, '_>) {
        let Node::LiquidFilter(filter) = node else { return };
        if !matches!(filter.name, "stylesheet_tag" | "script_tag" | "image_tag" | "img_tag") {
            return;
        }
        let Some(Node::LiquidVariable(parent)) = ancestors.last().copied() else { return };
        if parent.filters.iter().any(|f| matches!(f, Node::LiquidFilter(f) if ASSET_URL_FILTERS.contains(&f.name))) {
            return;
        }
        let url = parent.expression.position();
        if let Node::String(s) = &parent.expression {
            if is_data_uri(s.value) {
                return;
            }
            if !is_url_hosted_by_shopify(s.value, &self.allowed_domains) {
                cx.report(LiquidProblem::new(CDN_MESSAGE, url.start, url.end));
            }
        }
        cx.report(LiquidProblem::new(ASSET_MESSAGE, url.start, node.position().end));
    }
}

impl<'a> LiquidCheck<'a> for RemoteAsset {
    fn enter(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        match node {
            Node::LiquidFilter(_) => self.check_filter(node, ancestors, cx),
            _ => self.check_html_node(node, cx),
        }
    }
}

/// `new URL(url).hostname` with no base, `None` when the constructor throws: the
/// WHATWG URL parser up to the end of the authority, plus the port's validity.
///
/// Non-ASCII hosts only get the UTS #46 mappings in `map_to_ascii`, not full IDNA.
/// The result is only ever compared with domains, so a host differs from upstream
/// only if another mapping would have turned it into exactly one of those.
fn url_hostname(input: &str) -> Option<String> {
    let trimmed = input.trim_matches(|c: char| c <= ' ');
    let cleaned: Cow<'_, str> = if trimmed.contains(['\t', '\n', '\r']) {
        Cow::Owned(trimmed.replace(['\t', '\n', '\r'], ""))
    } else {
        Cow::Borrowed(trimmed)
    };
    let (scheme, rest) = cleaned.split_once(':')?;
    let mut scheme_bytes = scheme.bytes();
    if !scheme_bytes.next()?.is_ascii_alphabetic()
        || !scheme_bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    match scheme.as_str() {
        "file" => file_hostname(rest),
        "http" | "https" | "ws" | "wss" | "ftp" => {
            authority_hostname(rest.trim_start_matches(['/', '\\']), true)
        }
        _ => match rest.strip_prefix("//") {
            Some(authority) => authority_hostname(authority, false),
            None => Some(String::new()),
        },
    }
}

fn file_hostname(rest: &str) -> Option<String> {
    let bytes = rest.as_bytes();
    let slash = |i: usize| matches!(bytes.get(i), Some(b'/' | b'\\'));
    if !(slash(0) && slash(1)) {
        return Some(String::new());
    }
    let after = &rest[2..];
    let host = &after[..after.find(['/', '\\', '?', '#']).unwrap_or(after.len())];
    let host_bytes = host.as_bytes();
    // A Windows drive letter is a path, not a host.
    let is_drive = host_bytes.len() == 2 && host_bytes[0].is_ascii_alphabetic() && matches!(host_bytes[1], b':' | b'|');
    if host.is_empty() || is_drive {
        return Some(String::new());
    }
    let parsed = parse_host(host, true)?;
    Some(if parsed == "localhost" { String::new() } else { parsed })
}

fn authority_hostname(rest: &str, special: bool) -> Option<String> {
    let end = if special { rest.find(['/', '\\', '?', '#']) } else { rest.find(['/', '?', '#']) };
    let authority = &rest[..end.unwrap_or(rest.len())];
    let host_port = match authority.rfind('@') {
        Some(at) => {
            let after = &authority[at + 1..];
            if after.is_empty() {
                return None;
            }
            after
        }
        None => authority,
    };
    let mut in_brackets = false;
    let colon = host_port.char_indices().find_map(|(i, c)| {
        match c {
            '[' => in_brackets = true,
            ']' => in_brackets = false,
            ':' if !in_brackets => return Some(i),
            _ => {}
        }
        None
    });
    let host = &host_port[..colon.unwrap_or(host_port.len())];
    if host.is_empty() && (special || colon.is_some()) {
        return None;
    }
    if let Some(colon) = colon {
        let mut port = 0u32;
        for c in host_port[colon + 1..].chars() {
            port = port * 10 + c.to_digit(10)?;
            if port > 65535 {
                return None;
            }
        }
    }
    if host.is_empty() {
        return Some(String::new());
    }
    parse_host(host, special)
}

fn parse_host(host: &str, special: bool) -> Option<String> {
    if host.starts_with('[') {
        return host.ends_with(']').then(|| host.to_ascii_lowercase());
    }
    const FORBIDDEN: [char; 17] =
        ['\0', '\t', '\n', '\r', ' ', '#', '/', ':', '<', '>', '?', '@', '[', '\\', ']', '^', '|'];
    if !special {
        return if host.contains(FORBIDDEN) { None } else { Some(host.to_string()) };
    }
    let decoded = percent_decode(host);
    let domain = map_to_ascii(&decoded).to_ascii_lowercase();
    if domain.is_empty() || domain.contains(FORBIDDEN) || domain.contains(|c: char| c < ' ' || c == '%' || c == '\x7f') {
        return None;
    }
    if ends_in_number(&domain) {
        return parse_ipv4(&domain);
    }
    Some(domain)
}

/// The UTS #46 mappings that yield ASCII for the characters a lookalike of an ASCII
/// domain uses: fullwidth forms, the other full stops, a few letters, and the ignored
/// invisible characters.
fn map_to_ascii(domain: &str) -> Cow<'_, str> {
    if domain.is_ascii() {
        return Cow::Borrowed(domain);
    }
    let mut out = String::with_capacity(domain.len());
    for c in domain.chars() {
        match c {
            '\u{ff01}'..='\u{ff5e}' => out.extend(char::from_u32(c as u32 - 0xfee0)),
            '\u{3002}' | '\u{ff61}' => out.push('.'),
            '\u{17f}' => out.push('s'),
            '\u{212a}' => out.push('k'),
            '\u{ad}' | '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{fe00}'..='\u{fe0f}' | '\u{feff}' => {}
            _ => out.push(c),
        }
    }
    Cow::Owned(out)
}

fn percent_decode(input: &str) -> String {
    if !input.contains('%') {
        return input.to_string();
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
            if let Some(value) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                if hex.is_some_and(|h| h.bytes().all(|b| b.is_ascii_hexdigit())) {
                    out.push(value);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn ipv4_number(part: &str) -> Option<u64> {
    if part.is_empty() {
        return None;
    }
    let (digits, radix) = match part.as_bytes() {
        [b'0', b'x' | b'X', ..] => (&part[2..], 16),
        [b'0', _, ..] => (&part[1..], 8),
        _ => (part, 10),
    };
    let mut value = 0u64;
    for c in digits.chars() {
        value = value.saturating_mul(radix).saturating_add(u64::from(c.to_digit(radix as u32)?));
    }
    Some(value)
}

fn ends_in_number(domain: &str) -> bool {
    let mut parts: Vec<&str> = domain.split('.').collect();
    if parts.last() == Some(&"") {
        if parts.len() == 1 {
            return false;
        }
        parts.pop();
    }
    let last = parts[parts.len() - 1];
    (!last.is_empty() && last.bytes().all(|b| b.is_ascii_digit())) || ipv4_number(last).is_some()
}

fn parse_ipv4(domain: &str) -> Option<String> {
    let mut parts: Vec<&str> = domain.split('.').collect();
    if parts.last() == Some(&"") && parts.len() > 1 {
        parts.pop();
    }
    if parts.len() > 4 {
        return None;
    }
    let numbers = parts.iter().map(|p| ipv4_number(p)).collect::<Option<Vec<u64>>>()?;
    let (&last, init) = numbers.split_last()?;
    if init.iter().any(|&n| n > 255) || last >= 256u64.pow(5 - numbers.len() as u32) {
        return None;
    }
    let mut ipv4 = last;
    for (i, &n) in init.iter().enumerate() {
        ipv4 += n * 256u64.pow(3 - i as u32);
    }
    Some(format!("{}.{}.{}.{}", (ipv4 >> 24) & 255, (ipv4 >> 16) & 255, (ipv4 >> 8) & 255, ipv4 & 255))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected values were generated by running `new URL(input).hostname` in Node.
    #[test]
    fn hostnames_match_node() {
        let cases: &[(&str, Option<&str>)] = &[
            ("https://cdn.shopify.com/a", Some("cdn.shopify.com")),
            ("HTTPS://CDN.SHOPIFY.COM/a", Some("cdn.shopify.com")),
            ("https:cdn.shopify.com/a", Some("cdn.shopify.com")),
            ("https:/cdn.shopify.com/a", Some("cdn.shopify.com")),
            ("https:///cdn.shopify.com/a", Some("cdn.shopify.com")),
            ("https:\\\\cdn.shopify.com\\a", Some("cdn.shopify.com")),
            ("//cdn.shopify.com/a", None),
            ("ws://a.b/", Some("a.b")),
            ("ftp://a.b/", Some("a.b")),
            ("foo://cdn.shopify.com/a", Some("cdn.shopify.com")),
            ("foo://CDN.shopify.com/a", Some("CDN.shopify.com")),
            ("foo:cdn.shopify.com", Some("")),
            ("foo:/a/b", Some("")),
            ("foo://", Some("")),
            ("foo://:80", None),
            ("foo://a:80", Some("a")),
            ("foo://a b", None),
            ("file://a.b/c", Some("a.b")),
            ("file:///a", Some("")),
            ("file://localhost/a", Some("")),
            ("file://LOCALHOST/a", Some("")),
            ("file://c:/a", Some("")),
            ("file:a", Some("")),
            ("file:/a", Some("")),
            ("file:\\\\a.b\\c", Some("a.b")),
            ("https://u:p@a.b/x", Some("a.b")),
            ("https://x@y@a.b/x", Some("a.b")),
            ("https://@a.b", Some("a.b")),
            ("https://a.b@c.d/x", Some("c.d")),
            ("https://c.d@/x", None),
            ("https://a.b:443/x", Some("a.b")),
            ("https://a.b:65535/x", Some("a.b")),
            ("https://a.b:65536/x", None),
            ("https://a.b:abc/x", None),
            ("https://a.b:/x", Some("a.b")),
            ("https://:80/x", None),
            ("https://a%2Eb/x", Some("a.b")),
            ("https://a.b./x", Some("a.b.")),
            ("https://a.b?x", Some("a.b")),
            ("https://a.b#x", Some("a.b")),
            ("  https://a.b/x  ", Some("a.b")),
            ("https://a\u{9}.b/x", Some("a.b")),
            ("https://a b/x", None),
            ("https://a<b/x", None),
            ("https://a%b/x", None),
            ("https://a%25b/x", None),
            ("https://[::1]/x", Some("[::1]")),
            ("https://[::1/x", None),
            ("https://127.0.0.1/x", Some("127.0.0.1")),
            ("https://127.1/x", Some("127.0.0.1")),
            ("https://1.2.3.4.5/x", None),
            ("https://0x7f.1/x", Some("127.0.0.1")),
            ("https://0177.0.0.1/x", Some("127.0.0.1")),
            ("https://256.1.1.1/x", None),
            ("https://1.1.1.256/x", None),
            ("https://1.1.65536/x", None),
            ("https://4294967295/x", Some("255.255.255.255")),
            ("https://4294967296/x", None),
            ("https://a.1/x", None),
            ("https://1.a/x", Some("1.a")),
            ("https://0x/x", Some("0.0.0.0")),
            ("https://a.0x/x", None),
            ("https://1.2.3.4./x", Some("1.2.3.4")),
            ("https://", None),
            ("http:", None),
            ("a.js", None),
            ("1a:b", None),
            ("a+b.c-d:x", Some("")),
            ("#x", None),
            ("", None),
            ("mailto:a@b", Some("")),
            ("https://\u{ff43}\u{ff44}\u{ff4e}.shopify.com/a", Some("cdn.shopify.com")),
            ("https://cdn\u{3002}shopify.com/a", Some("cdn.shopify.com")),
            ("https://cdn.\u{17f}hopify.com/a", Some("cdn.shopify.com")),
            ("https://cdn.sho\u{ad}pify.com/a", Some("cdn.shopify.com")),
            ("data:x", Some("")),
            ("https://a.b\\c/x", Some("a.b")),
            ("https://a.b/c@d", Some("a.b")),
        ];
        for &(input, want) in cases {
            assert_eq!(url_hostname(input).as_deref(), want, "new URL({input:?}).hostname");
        }
    }
}
