//! Page assembly, as Shopify's renderer does it: the template's sections render first, each in
//! its own context, then the layout renders with `content_for_layout`.

use crate::cost::CostModel;
use crate::interp::{Container, Limits, Renderer, Scope};
use crate::shop::Shop;
use crate::theme::Theme;
use crate::value::{from_json, Object, Value};
use std::collections::HashMap;
use std::rc::Rc;

/// The page types `lq profile` can render, with the template each uses and its `request.page_type`.
pub const PAGE_TYPES: &[&str] = &[
    "index", "product", "collection", "list-collections", "cart", "search", "page", "blog", "article", "404",
];

#[derive(Clone, Debug)]
pub struct PageRequest {
    /// `product`, `collection`, ... or `product.alternate` for a template suffix.
    pub template: String,
    /// Handle of the product, collection, page, blog or article to render. Defaults to the first.
    pub handle: Option<String>,
}

pub struct PageResult {
    pub html: String,
    pub profile: crate::profile::Profile,
    pub errors: Vec<String>,
    pub unsupported: Vec<String>,
    pub loop_sources: HashMap<String, String>,
    pub template_path: String,
}

pub fn render_page(
    theme: &Theme,
    shop: &Shop,
    req: &PageRequest,
    model: &CostModel,
    limits: Limits,
    loop_shapes: &HashMap<String, usize>,
) -> Result<PageResult, String> {
    let (kind, suffix) = match req.template.split_once('.') {
        Some((k, s)) => (k.to_string(), Some(s.to_string())),
        None => (req.template.clone(), None),
    };
    let base = format!("templates/{}", req.template);
    let template_path = [format!("{base}.json"), format!("{base}.liquid")]
        .into_iter()
        .find(|p| theme.get(p).is_some())
        .ok_or_else(|| format!("the theme has no {base}.json or {base}.liquid"))?;

    let mut r = Renderer::new(theme, model, &format!("{} (lq estimate)", req.template), limits);
    r.loop_shapes = loop_shapes.clone();
    set_globals(&mut r, shop, &kind, suffix.as_deref(), req.handle.as_deref())?;

    // The template: a JSON template's sections, or a Liquid template.
    let mut content = String::new();
    if template_path.ends_with(".json") {
        let json = theme.json(&template_path).ok_or_else(|| format!("{template_path} isn't valid JSON"))?;
        if let Some(layout) = json.get("layout") {
            r.layout = Some(match layout {
                lq_json::Value::Bool(false) => "none".into(),
                other => other.as_str().unwrap_or("theme").to_string(),
            });
        }
        r.open_plain("json_template", "");
        render_section_list(&mut r, &json, "template", &mut content);
        r.profile.close();
    } else {
        r.render_template(&template_path, false, &mut content);
    }

    let layout = r.layout.clone().unwrap_or_else(|| "theme".into());
    let mut html = String::new();
    if layout == "none" {
        html = content;
    } else {
        r.set_global("content_for_layout", Value::str(&content));
        let path = format!("layout/{layout}.liquid");
        if !r.render_template(&path, false, &mut html) {
            return Err(format!("the theme has no {path}"));
        }
    }
    let unsupported = r.unsupported.iter().cloned().collect();
    let loop_sources = std::mem::take(&mut r.loop_sources);
    Ok(PageResult { html, errors: r.errors.clone(), unsupported, loop_sources, profile: r.profile, template_path })
}

fn handle_of(v: &Value) -> String {
    match v {
        Value::Object(o) => o.get("handle").map(|h| h.render()).unwrap_or_default(),
        _ => String::new(),
    }
}

fn pick(list: &[Value], handle: Option<&str>) -> Value {
    match handle {
        Some(h) => list.iter().find(|v| handle_of(v) == h).cloned().unwrap_or_default(),
        None => list.first().cloned().unwrap_or_default(),
    }
}

/// An accessor object such as `collections` or `all_products`: any handle reads a database-backed
/// property. Unknown handles resolve to a stand-in, so a theme's real handles still render.
fn accessor(ty: &str, list: &[Value]) -> Value {
    let mut o = Object::new(ty);
    for v in list {
        o.set(&handle_of(v), v.clone());
    }
    o.set("__items", Value::array(list.to_vec()));
    o.set("size", Value::Int(list.len() as i64));
    Value::object(o)
}

fn set_globals(r: &mut Renderer<'_>, shop: &Shop, kind: &str, suffix: Option<&str>, handle: Option<&str>) -> Result<(), String> {
    for (k, v) in &shop.globals {
        r.set_global(k, v.clone());
    }
    r.set_global("collections", accessor("collections", &shop.collections));
    r.set_global("all_products", accessor("all_products", &shop.products));
    r.set_global("pages", accessor("pages", &shop.pages));
    r.set_global("blogs", accessor("blogs", &shop.blogs));
    r.set_global("articles", accessor("articles", &shop.articles));
    r.set_global("linklists", accessor("linklists", &shop.linklists));
    r.set_global("metaobjects", accessor("metaobjects", &[]));
    r.set_global("images", accessor("images", &[]));

    let mut template = Object::new("template");
    template.set("name", Value::str(kind));
    template.set("suffix", suffix.map(Value::str).unwrap_or_default());
    template.set("directory", Value::Nil);
    template.set("__to_s", Value::str(match suffix {
        Some(s) => format!("{kind}.{s}"),
        None => kind.to_string(),
    }));
    r.set_global("template", Value::object(template));

    let path = match kind {
        "index" => "/".to_string(),
        "product" => format!("/products/{}", handle_of(&pick(&shop.products, handle))),
        "collection" => format!("/collections/{}", handle_of(&pick(&shop.collections, handle))),
        "list-collections" => "/collections".into(),
        "cart" => "/cart".into(),
        "search" => "/search?q=parka".into(),
        "page" => format!("/pages/{}", handle_of(&pick(&shop.pages, handle))),
        "blog" => "/blogs/news".into(),
        "article" => format!("/blogs/news/{}", handle_of(&pick(&shop.articles, handle))),
        _ => format!("/{kind}"),
    };
    let mut locale = Object::new("shop_locale");
    locale.set("iso_code", Value::str("en"));
    locale.set("name", Value::str("English"));
    locale.set("endonym_name", Value::str("English"));
    locale.set("primary", Value::Bool(true));
    locale.set("root_url", Value::str("/"));
    locale.set("__to_s", Value::str("en"));
    let mut request = Object::new("request");
    request.set("page_type", Value::str(kind));
    request.set("path", Value::str(&path));
    request.set("host", Value::str("northbound.example"));
    request.set("origin", Value::str("https://northbound.example"));
    request.set("locale", Value::object(locale));
    request.set("design_mode", Value::Bool(false));
    request.set("visual_preview_mode", Value::Bool(false));
    r.set_global("request", Value::object(request));

    let mut routes = Object::new("routes");
    for (k, v) in [
        ("root_url", "/"), ("account_url", "/account"), ("account_login_url", "/account/login"),
        ("account_logout_url", "/account/logout"), ("account_register_url", "/account/register"),
        ("account_addresses_url", "/account/addresses"), ("collections_url", "/collections"),
        ("all_products_collection_url", "/collections/all"), ("search_url", "/search"),
        ("predictive_search_url", "/search/suggest"), ("cart_url", "/cart"), ("cart_add_url", "/cart/add"),
        ("cart_change_url", "/cart/change"), ("cart_clear_url", "/cart/clear"), ("cart_update_url", "/cart/update"),
        ("product_recommendations_url", "/recommendations/products"),
    ] {
        routes.set(k, Value::str(v));
    }
    r.set_global("routes", Value::object(routes));
    r.set_global("canonical_url", Value::str(format!("https://northbound.example{path}")));
    r.set_global("page_title", Value::str("Northbound"));
    r.set_global("page_description", Value::str("Outerwear for cold places."));
    r.set_global("handle", Value::str(handle.unwrap_or("")));
    r.set_global("current_page", Value::Int(1));
    r.set_global("current_tags", Value::Nil);
    r.set_global("powered_by_link", Value::str("<a href=\"https://www.shopify.com\">Powered by Shopify</a>"));
    r.set_global("content_for_header", Value::str(CONTENT_FOR_HEADER));
    r.set_global("content_for_index", Value::str(""));
    r.set_global("additional_checkout_buttons", Value::Bool(false));
    r.set_global("content_for_additional_checkout_buttons", Value::str(""));
    r.set_global("scripts", Value::array(Vec::new()));
    r.set_global("predictive_search", obj("predictive_search", &[("performed", Value::Bool(false))]));
    let settings = global_settings(r);
    r.set_global("settings", settings);

    match kind {
        "product" => {
            let p = pick(&shop.products, handle);
            if p.is_nil() {
                return Err(format!("the mock shop has no product '{}'", handle.unwrap_or("")));
            }
            r.set_global("product", p);
            let recs: Vec<Value> = shop.products.iter().skip(1).take(4).cloned().collect();
            r.set_global("recommendations", obj("recommendations", &[
                ("performed", Value::Bool(true)),
                ("products_count", Value::Int(recs.len() as i64)),
                ("products", Value::array(recs)),
                ("intent", Value::str("related")),
            ]));
        }
        "collection" => {
            let c = pick(&shop.collections, handle);
            if c.is_nil() {
                return Err(format!("the mock shop has no collection '{}'", handle.unwrap_or("")));
            }
            r.set_global("collection", c);
        }
        "page" => r.set_global("page", pick(&shop.pages, handle)),
        "blog" => r.set_global("blog", pick(&shop.blogs, None)),
        "article" => {
            r.set_global("blog", pick(&shop.blogs, None));
            r.set_global("article", pick(&shop.articles, handle));
        }
        "search" => {
            let results: Vec<Value> = shop.products.iter().take(24).cloned().collect();
            let filters = match shop.collections.first() {
                Some(Value::Object(c)) => c.get("filters").cloned().unwrap_or_default(),
                _ => Value::Nil,
            };
            r.set_global("search", obj("search", &[
                ("performed", Value::Bool(true)),
                ("terms", Value::str("parka")),
                ("results_count", Value::Int(results.len() as i64)),
                ("results", Value::array(results)),
                ("types", Value::array(vec![Value::str("product")])),
                ("filters", filters),
                ("sort_by", Value::str("relevance")),
                ("default_sort_by", Value::str("relevance")),
                ("sort_options", Value::array(Vec::new())),
            ]));
        }
        _ => {}
    }
    // `closest.<type>`: the nearest resource of that type, which at page level is the page's own.
    let mut closest = Object::new("");
    for k in ["product", "collection", "article", "blog", "page", "metaobject"] {
        if let Some(v) = r.globals.get(k).cloned() {
            closest.set(k, v);
        }
    }
    r.set_global("closest", Value::object(closest));
    Ok(())
}

fn obj(ty: &str, fields: &[(&str, Value)]) -> Value {
    let mut o = Object::new(ty);
    for (k, v) in fields {
        o.set(k, v.clone());
    }
    Value::object(o)
}

/// About the size Shopify injects: scripts, preloads and analytics.
const CONTENT_FOR_HEADER: &str = concat!(
    "<script>window.performance && window.performance.mark && window.performance.mark('shopify.content_for_header.start');</script>",
    "<meta id=\"shopify-digital-wallet\" name=\"shopify-digital-wallet\" content=\"/1/digital_wallets/dialog\">",
    "<script id=\"shopify-features\" type=\"application/json\">{\"accessToken\":\"x\",\"betas\":[\"rich-media-storefront-analytics\"],\"domain\":\"northbound.example\",\"predictiveSearch\":true,\"shopId\":1,\"locale\":\"en\"}</script>",
    "<script>var Shopify = Shopify || {};Shopify.shop = \"northbound.myshopify.com\";Shopify.locale = \"en\";Shopify.currency = {\"active\":\"USD\",\"rate\":\"1.0\"};</script>",
    "<script>window.performance && window.performance.mark && window.performance.mark('shopify.content_for_header.end');</script>",
);

// ---- settings ----------------------------------------------------------------------------------

fn global_settings(r: &mut Renderer<'_>) -> Value {
    let theme = r.theme;
    let mut defs: Vec<(String, String, Option<lq_json::Value>)> = Vec::new();
    if let Some(lq_json::Value::Array(groups)) = theme.json("config/settings_schema.json") {
        for g in groups {
            if let Some(lq_json::Value::Array(settings)) = g.get("settings") {
                for s in settings {
                    if let (Some(id), Some(ty)) = (s.get("id").and_then(|v| v.as_str()), s.get("type").and_then(|v| v.as_str())) {
                        defs.push((id.to_string(), ty.to_string(), s.get("default").cloned()));
                    }
                }
            }
        }
    }
    let data = theme.json("config/settings_data.json");
    let current = data.as_ref().and_then(|d| match d.get("current") {
        Some(lq_json::Value::String(preset)) => d.get("presets").and_then(|p| p.get(preset)).cloned(),
        other => other.cloned(),
    });
    let mut settings = Object::new("settings");
    for (id, ty, default) in &defs {
        let raw = current.as_ref().and_then(|c| c.get(id)).cloned().or_else(|| default.clone());
        let v = resolve_setting(r, ty, raw.as_ref());
        settings.set(id, v);
    }
    // Settings with no schema entry, such as color_schemes, still come through.
    if let Some(lq_json::Value::Object(c)) = &current {
        for (k, v) in c.iter() {
            if settings.get(k).is_none() && k != "sections" && k != "content_for_index" {
                let val = if k == "color_schemes" { color_schemes(v) } else { from_json(v) };
                settings.set(k, val);
            }
        }
    }
    Value::object(settings)
}

fn color_schemes(v: &lq_json::Value) -> Value {
    let mut group = Object::new("color_scheme_group");
    let mut items = Vec::new();
    if let lq_json::Value::Object(o) = v {
        for (id, scheme) in o.iter() {
            let mut s = Object::new("color_scheme");
            s.set("id", Value::str(id));
            s.set("settings", scheme.get("settings").map(from_json).unwrap_or_default());
            s.set("__to_s", Value::str(id));
            let s = Value::object(s);
            group.set(id, s.clone());
            items.push(s);
        }
    }
    group.set("__items", Value::array(items));
    Value::object(group)
}

/// Shopify resolves a setting's stored value to an object by its type.
pub(crate) fn resolve_setting(r: &mut Renderer<'_>, ty: &str, raw: Option<&lq_json::Value>) -> Value {
    let Some(raw) = raw else {
        return match ty {
            "checkbox" => Value::Bool(false),
            "product_list" | "collection_list" | "metaobject_list" => Value::array(Vec::new()),
            _ => Value::Nil,
        };
    };
    let s = raw.as_str().unwrap_or("");
    if s.starts_with("{{") && s.ends_with("}}") {
        return dynamic_source(r, s);
    }
    match ty {
        "image_picker" => {
            if s.is_empty() {
                return Value::Nil;
            }
            let file = s.rsplit('/').next().unwrap_or(s);
            let src = format!("//cdn.shopify.com/s/files/1/0000/0001/files/{file}?v=1700000000");
            obj("image", &[
                ("src", Value::str(&src)), ("width", Value::Int(1600)), ("height", Value::Int(1200)),
                ("aspect_ratio", Value::Float(1.333)), ("alt", Value::str("")), ("id", Value::Int(1)),
                ("presentation", obj("", &[("focal_point", Value::str("50.0% 50.0%"))])),
                ("__to_s", Value::str(&src)),
            ])
        }
        "collection" => resolve_handle(r, "collections", s),
        "product" => resolve_handle(r, "all_products", s),
        "blog" => resolve_handle(r, "blogs", s),
        "page" => resolve_handle(r, "pages", s),
        "article" => resolve_handle(r, "articles", s),
        "link_list" => resolve_handle(r, "linklists", s),
        "product_list" | "collection_list" => {
            let acc = if ty == "product_list" { "all_products" } else { "collections" };
            let list = match raw {
                lq_json::Value::Array(a) => a.iter().map(|h| resolve_handle(r, acc, h.as_str().unwrap_or(""))).collect(),
                _ => Vec::new(),
            };
            Value::array(list)
        }
        "font_picker" => {
            let family = s.split('_').next().unwrap_or("assistant");
            let mut family = family.to_string();
            if let Some(f) = family.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            obj("font", &[
                ("family", Value::str(&family)), ("fallback_families", Value::str("sans-serif")),
                ("weight", Value::Int(400)), ("style", Value::str("normal")), ("system?", Value::Bool(false)),
                ("variants", Value::array(Vec::new())), ("__to_s", Value::str(s)),
            ])
        }
        "color_scheme" => {
            let schemes = r.globals.get("settings").cloned();
            let found = match &schemes {
                Some(Value::Object(set)) => match set.get("color_schemes") {
                    Some(Value::Object(g)) => g.get(s).cloned(),
                    _ => None,
                },
                _ => None,
            };
            found.unwrap_or_else(|| obj("color_scheme", &[("id", Value::str(s)), ("__to_s", Value::str(s))]))
        }
        "video_url" => {
            if s.is_empty() {
                Value::Nil
            } else {
                obj("video_url", &[("id", Value::str("abc")), ("type", Value::str("youtube")), ("__to_s", Value::str(s))])
            }
        }
        "url" if s.is_empty() => Value::Nil,
        _ => from_json(raw),
    }
}

/// Settings store real handles; the mock shop has its own. Known handles resolve to themselves and
/// unknown ones to a stand-in picked by hash, so the page renders the same amount of content.
fn resolve_handle(r: &mut Renderer<'_>, accessor: &str, handle: &str) -> Value {
    if handle.is_empty() {
        return Value::Nil;
    }
    let acc = r.globals.get(accessor).cloned().unwrap_or_default();
    let key = Value::str(handle);
    let direct = r.get_prop(&acc, &key);
    if !direct.is_nil() {
        return direct;
    }
    if let Value::Object(o) = &acc {
        if let Some(Value::Array(items)) = o.get("__items") {
            if !items.is_empty() {
                let h = handle.bytes().fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize));
                return items[h % items.len()].clone();
            }
        }
    }
    Value::Nil
}

/// `{{ closest.product.title }}` and `{{ product.metafields.x.y }}` style dynamic sources.
fn dynamic_source(r: &mut Renderer<'_>, s: &str) -> Value {
    let inner = s.trim_start_matches("{{").trim_end_matches("}}").trim();
    let path = inner.split('|').next().unwrap_or("").trim();
    let mut parts = path.split('.').peekable();
    let Some(first) = parts.next() else { return Value::Nil };
    let mut v = r.lookup_var(first);
    if first == "closest" {
        let name = parts.next().unwrap_or("");
        let near = r.get_prop(&v, &Value::str(name));
        v = if near.is_nil() { r.lookup_var(name) } else { near };
    }
    for p in parts {
        v = r.get_prop(&v, &Value::str(p));
    }
    v
}

// ---- schemas -----------------------------------------------------------------------------------

/// The `{% schema %}` JSON of a section or block file.
fn schema(theme: &Theme, path: &str) -> Option<lq_json::Value> {
    let f = theme.get(path)?;
    let src = f.source;
    let start = src.find("{% schema %}").or_else(|| src.find("{%- schema -%}")).or_else(|| src.find("{% schema -%}"))?;
    let body_start = src[start..].find("%}")? + start + 2;
    let end = src[body_start..].find("{%")? + body_start;
    lq_json::parse_json(&src[body_start..end])
}

fn schema_settings(schema: Option<&lq_json::Value>) -> Vec<(String, String, Option<lq_json::Value>)> {
    let mut out = Vec::new();
    if let Some(lq_json::Value::Array(settings)) = schema.and_then(|s| s.get("settings")) {
        for s in settings {
            if let (Some(id), Some(ty)) = (s.get("id").and_then(|v| v.as_str()), s.get("type").and_then(|v| v.as_str())) {
                out.push((id.to_string(), ty.to_string(), s.get("default").cloned()));
            }
        }
    }
    out
}

fn build_settings(r: &mut Renderer<'_>, defs: &[(String, String, Option<lq_json::Value>)], data: Option<&lq_json::Value>) -> Value {
    let mut settings = Object::new("");
    for (id, ty, default) in defs {
        let raw = data.and_then(|d| d.get(id)).cloned().or_else(|| default.clone());
        let v = resolve_setting(r, ty, raw.as_ref());
        settings.set(id, v);
    }
    if let Some(lq_json::Value::Object(d)) = data {
        for (k, v) in d.iter() {
            if settings.get(k).is_none() {
                settings.set(k, from_json(v));
            }
        }
    }
    Value::object(settings)
}

fn ordered<'a>(json: &'a lq_json::Value, map_key: &str, order_key: &str) -> Vec<(String, &'a lq_json::Value)> {
    let Some(lq_json::Value::Object(map)) = json.get(map_key) else { return Vec::new() };
    let order: Vec<String> = match json.get(order_key) {
        Some(lq_json::Value::Array(o)) => o.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
        _ => map.keys().map(String::from).collect(),
    };
    order
        .into_iter()
        .filter_map(|id| map.get(&id).map(|v| (id, v)))
        .filter(|(_, v)| !v.get("disabled").and_then(|d| d.as_bool()).unwrap_or(false))
        .collect()
}

/// A block from template JSON, with its settings and nested blocks.
fn build_block(r: &mut Renderer<'_>, id: &str, data: &lq_json::Value, section_schema: Option<&lq_json::Value>, index: usize) -> Value {
    let ty = data.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
    // A theme block's schema lives in blocks/<type>.liquid; a section block's in the section schema.
    let file_schema = schema(r.theme, &format!("blocks/{ty}.liquid"));
    let local = section_schema.and_then(|s| match s.get("blocks") {
        Some(lq_json::Value::Array(b)) => b.iter().find(|b| b.get("type").and_then(|t| t.as_str()) == Some(ty.as_str())).cloned(),
        _ => None,
    });
    let defs = schema_settings(file_schema.as_ref().or(local.as_ref()));
    let settings = build_settings(r, &defs, data.get("settings"));
    let children: Vec<Value> = ordered(data, "blocks", "block_order")
        .into_iter()
        .enumerate()
        .map(|(i, (cid, c))| build_block(r, &cid, c, file_schema.as_ref(), i))
        .collect();
    let mut b = Object::new("block");
    b.set("id", Value::str(id));
    b.set("type", Value::str(&ty));
    b.set("settings", settings);
    b.set("shopify_attributes", Value::str(format!("data-shopify-editor-block=\"{{&quot;id&quot;:&quot;{id}&quot;}}\"")));
    b.set("index", Value::Int(index as i64 + 1));
    b.set("__blocks", Value::array(children));
    b.set("__static", Value::Bool(data.get("static").and_then(|s| s.as_bool()).unwrap_or(false)));
    Value::object(b)
}

fn block_children(v: &Value) -> Vec<Value> {
    match v {
        Value::Object(o) => match o.get("__blocks") {
            Some(Value::Array(a)) => a.as_ref().clone(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Renders one section from template or group JSON.
/// Sections from JSON render under a `raw_section` frame, which also holds resolving their
/// settings; a `{% section %}` tag's doesn't.
fn render_section(r: &mut Renderer<'_>, id: &str, data: &lq_json::Value, location: &str, index: usize, raw: bool, out: &mut String) {
    let ty = data.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
    if ty.starts_with("_blocks") && r.theme.get(&format!("sections/{ty}.liquid")).is_none() {
        return;
    }
    if raw {
        let file = r.current_template();
        r.open_plain("raw_section", &file);
    }
    render_section_inner(r, id, &ty, data, location, index, out);
    if raw {
        r.profile.close();
    }
}

fn render_section_inner(r: &mut Renderer<'_>, id: &str, ty: &str, data: &lq_json::Value, location: &str, index: usize, out: &mut String) {
    let ty = ty.to_string();
    let path = format!("sections/{ty}.liquid");
    let schema = schema(r.theme, &path);
    let defs = schema_settings(schema.as_ref());
    let settings = build_settings(r, &defs, data.get("settings"));
    let blocks: Vec<Value> = ordered(data, "blocks", "block_order")
        .into_iter()
        .filter(|(_, b)| !b.get("type").and_then(|t| t.as_str()).unwrap_or("").starts_with("@app"))
        .enumerate()
        .map(|(i, (bid, b))| build_block(r, &bid, b, schema.as_ref(), i))
        .collect();
    let mut s = Object::new("section");
    s.set("id", Value::str(id));
    s.set("settings", settings);
    s.set("blocks", Value::array(blocks.clone()));
    s.set("location", Value::str(location));
    s.set("index", Value::Int(index as i64 + 1));
    s.set("index0", Value::Int(index as i64));
    let section = Value::object(s);
    render_section_value(r, &ty, id, section, blocks, out);
}

fn render_section_value(r: &mut Renderer<'_>, ty: &str, id: &str, section: Value, blocks: Vec<Value>, out: &mut String) {
    let path = format!("sections/{ty}.liquid");
    if r.theme.get(&path).is_none() {
        r.errors.push(format!("section '{id}' has type '{ty}', but {path} doesn't exist"));
        return;
    }
    let saved_scopes = std::mem::replace(&mut r.scopes, vec![Scope::new()]);
    let saved_section = r.globals.insert(Rc::from("section"), section.clone());
    r.containers.push(Container { section: section.clone(), blocks });
    out.push_str(&format!("<div id=\"shopify-section-{id}\" class=\"shopify-section\">"));
    let _ = r.with_depth(|r| r.render_template(&path, true, out));
    out.push_str("</div>");
    r.containers.pop();
    match saved_section {
        Some(s) => r.globals.insert(Rc::from("section"), s),
        None => r.globals.remove("section"),
    };
    r.scopes = saved_scopes;
}

fn render_section_list(r: &mut Renderer<'_>, json: &lq_json::Value, location: &str, out: &mut String) {
    for (i, (id, data)) in ordered(json, "sections", "order").into_iter().enumerate() {
        render_section(r, &id, data, location, i, true, out);
    }
}

/// `{% sections 'header-group' %}`.
pub(crate) fn render_section_group(r: &mut Renderer<'_>, name: &str, out: &mut String) {
    let path = format!("sections/{name}.json");
    let Some(json) = r.theme.json(&path) else {
        r.errors.push(format!("{path} doesn't exist"));
        return;
    };
    let location = json.get("type").and_then(|t| t.as_str()).unwrap_or(name).to_string();
    let file = r.current_template();
    r.open_plain("json_section_group", &file);
    render_section_list(r, &json, &location, out);
    r.profile.close();
}

/// `{% section 'name' %}`: a static section, with settings from settings_data.json.
pub(crate) fn render_static_section(r: &mut Renderer<'_>, name: &str, out: &mut String) {
    let data = r.theme.json("config/settings_data.json");
    let stored = data.as_ref().and_then(|d| d.get("current")).and_then(|c| c.get("sections")).and_then(|s| s.get(name)).cloned();
    let mut json = lq_json::Object::new();
    json.insert("type", lq_json::Value::String(name.to_string()));
    let data = match stored {
        Some(lq_json::Value::Object(o)) => {
            let mut o = o.clone();
            o.insert("type", lq_json::Value::String(name.to_string()));
            lq_json::Value::Object(o)
        }
        _ => lq_json::Value::Object(json),
    };
    render_section(r, name, &data, "static", 0, false, out);
}

/// `{% content_for 'blocks' %}` and `{% content_for 'block', type: ..., id: ... %}`.
pub(crate) fn render_content_for(r: &mut Renderer<'_>, kind: &str, args: &[(String, Value)], offset: u32, out: &mut String) {
    let Some(container) = r.containers.last().cloned() else { return };
    let blocks: Vec<Value> = match kind {
        "blocks" => container.blocks.iter().filter(|b| !matches!(b, Value::Object(o) if o.get("__static").is_some_and(Value::is_truthy))).cloned().collect(),
        "block" => {
            let ty = args.iter().find(|(k, _)| k == "type").map(|(_, v)| v.render()).unwrap_or_default();
            let id = args.iter().find(|(k, _)| k == "id").map(|(_, v)| v.render()).unwrap_or_default();
            let found = container.blocks.iter().find(|b| matches!(b, Value::Object(o) if o.get("id").map(|i| i.render()) == Some(id.clone()))).cloned();
            match found {
                Some(b) => vec![b],
                None => {
                    let mut data = lq_json::Object::new();
                    data.insert("type", lq_json::Value::String(ty));
                    let data = lq_json::Value::Object(data);
                    vec![build_block(r, &id, &data, None, 0)]
                }
            }
        }
        _ => return,
    };
    for block in blocks {
        let ty = match &block {
            Value::Object(o) => o.get("type").map(|t| t.render()).unwrap_or_default(),
            _ => continue,
        };
        if ty.starts_with('@') {
            continue;
        }
        render_block_with(r, &ty, block, args, offset, out);
    }
}

/// Renders one block: its file from `blocks/`, with `block` set. App blocks (`@app`) aren't in
/// the theme, so they render nothing.
pub(crate) fn render_block_value(r: &mut Renderer<'_>, ty: &str, block: Value, offset: u32, out: &mut String) {
    if r.theme.get(&format!("blocks/{ty}.liquid")).is_none() {
        return;
    }
    render_block_with(r, ty, block, &[], offset, out);
}

fn render_block_with(r: &mut Renderer<'_>, ty: &str, block: Value, args: &[(String, Value)], offset: u32, out: &mut String) {
    if ty.starts_with('@') {
        return;
    }
    let Some(container) = r.containers.last().cloned() else { return };
    let path = format!("blocks/{ty}.liquid");
    if r.theme.get(&path).is_none() {
        r.error(offset, &format!("Could not find block {path}"), out);
        return;
    }
    r.count("block", 1.0);
    let children = block_children(&block);
    let saved_scopes = std::mem::replace(&mut r.scopes, vec![Scope::new()]);
    let saved_block = r.globals.insert(Rc::from("block"), block.clone());
    r.containers.push(Container { section: container.section.clone(), blocks: children });
    let mut args_scope = Scope::new();
    let mut closest = Object::new("");
    let outer = r.lookup_var("closest");
    if let Value::Object(o) = &outer {
        for (k, v) in o.fields.iter() {
            closest.set(k, v.clone());
        }
    }
    for (k, v) in args {
        if k == "type" || k == "id" {
            continue;
        }
        // `closest.product: product` sets what `{{ closest.product }}` dynamic sources read.
        match k.strip_prefix("closest.") {
            Some(name) => closest.set(name, v.clone()),
            None => {
                args_scope.insert(Rc::from(k.as_str()), v.clone());
            }
        }
    }
    if !closest.fields.is_empty() {
        args_scope.insert(Rc::from("closest"), Value::object(closest));
    }
    r.scopes = vec![args_scope];
    let _ = r.with_depth(|r| r.render_template(&path, true, out));
    r.containers.pop();
    match saved_block {
        Some(b) => r.globals.insert(Rc::from("block"), b),
        None => r.globals.remove("block"),
    };
    r.scopes = saved_scopes;
}
