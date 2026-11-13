//! Filters. Standard Liquid filters behave as in the Liquid gem. Shopify's own filters return
//! plausible stand-ins of the right type and size: what matters for profiling is that later
//! filters and tags see realistic input, not that a URL is real.

use crate::interp::Renderer;
use crate::value::{to_json, Value};

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or_default()
}

fn kwarg<'a>(kwargs: &'a [(String, Value)], name: &str) -> Option<&'a Value> {
    kwargs.iter().find(|(k, _)| k == name).map(|(_, v)| v)
}

fn num_op(a: &Value, b: &Value, int: fn(i64, i64) -> Option<i64>, float: fn(f64, f64) -> f64) -> Value {
    match (a.to_number(), b.to_number()) {
        (Value::Int(x), Value::Int(y)) => int(x, y).map(Value::Int).unwrap_or(Value::Nil),
        (x, y) => Value::Float(float(x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0))),
    }
}

fn s(v: &Value) -> String {
    v.render()
}

fn escape_html(t: &str) -> String {
    let mut o = String::with_capacity(t.len());
    for c in t.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            _ => o.push(c),
        }
    }
    o
}

fn handleize(t: &str) -> String {
    let mut o = String::new();
    let mut dash = false;
    for c in t.to_lowercase().chars() {
        if c.is_alphanumeric() {
            o.push(c);
            dash = false;
        } else if !dash && !o.is_empty() {
            o.push('-');
            dash = true;
        }
    }
    o.trim_end_matches('-').to_string()
}

fn money(cents: &Value, with_currency: bool, trailing: bool) -> Value {
    let c = cents.as_f64().unwrap_or(0.0);
    let amount = c / 100.0;
    let mut text = format!("${amount:.2}");
    if !trailing && text.ends_with(".00") {
        text.truncate(text.len() - 3);
    }
    if with_currency {
        text.push_str(" USD");
    }
    Value::str(text)
}

/// The URL of an image-like value: an image object's `src`, or the string itself.
fn image_src(v: &Value) -> String {
    match v {
        Value::Object(o) => o
            .get("src")
            .or_else(|| o.get("preview_image").and_then(|p| match p {
                Value::Object(p) => p.get("src"),
                _ => None,
            }))
            .or_else(|| o.get("featured_image").and_then(|p| match p {
                Value::Object(p) => p.get("src"),
                _ => None,
            }))
            .map(|x| x.render())
            .unwrap_or_else(|| "//cdn.shopify.com/s/files/1/0000/0001/files/placeholder.jpg".into()),
        other => {
            let t = other.render();
            if t.is_empty() { "//cdn.shopify.com/s/files/1/0000/0001/files/placeholder.jpg".into() } else { t }
        }
    }
}

fn with_params(url: &str, kwargs: &[(String, Value)]) -> String {
    let mut out = url.to_string();
    for (k, v) in kwargs {
        out.push(if out.contains('?') { '&' } else { '?' });
        out.push_str(&format!("{k}={}", v.render()));
    }
    out
}

fn sort_key(v: &Value, prop: Option<&str>, r: &mut Renderer<'_>) -> Value {
    match prop {
        Some(p) => r.get_prop(v, &Value::str(p)),
        None => v.clone(),
    }
}

pub fn apply(
    r: &mut Renderer<'_>,
    name: &str,
    input: Value,
    args: Vec<Value>,
    kwargs: Vec<(String, Value)>,
) -> Value {
    let a0 = arg(&args, 0);
    match name {
        // ---- strings ----
        "append" => Value::str(s(&input) + &s(&a0)),
        "prepend" => Value::str(s(&a0) + &s(&input)),
        "upcase" => Value::str(s(&input).to_uppercase()),
        "downcase" => Value::str(s(&input).to_lowercase()),
        "capitalize" => {
            let t = s(&input);
            let mut c = t.chars();
            Value::str(match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            })
        }
        "strip" => Value::str(s(&input).trim()),
        "lstrip" => Value::str(s(&input).trim_start()),
        "rstrip" => Value::str(s(&input).trim_end()),
        "strip_newlines" => Value::str(s(&input).replace(['\n', '\r'], "")),
        "newline_to_br" => Value::str(s(&input).replace('\n', "<br />\n")),
        "escape" | "h" => {
            if input.is_nil() { Value::Nil } else { Value::str(escape_html(&s(&input))) }
        }
        "escape_once" => Value::str(escape_html(&s(&input).replace("&amp;", "&"))),
        "url_encode" | "url_escape" | "url_param_escape" => Value::str(
            s(&input)
                .bytes()
                .map(|b| match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                    b' ' => "+".into(),
                    _ => format!("%{b:02X}"),
                })
                .collect::<String>(),
        ),
        "url_decode" => Value::str(s(&input).replace('+', " ")),
        "strip_html" => {
            let t = s(&input);
            let mut o = String::new();
            let mut in_tag = false;
            for c in t.chars() {
                match c {
                    '<' => in_tag = true,
                    '>' => in_tag = false,
                    _ if !in_tag => o.push(c),
                    _ => {}
                }
            }
            Value::str(o)
        }
        "replace" => Value::str(s(&input).replace(&s(&a0), &s(&arg(&args, 1)))),
        "replace_first" => Value::str(s(&input).replacen(&s(&a0), &s(&arg(&args, 1)), 1)),
        "replace_last" => {
            let t = s(&input);
            let pat = s(&a0);
            Value::str(match t.rfind(&pat) {
                Some(i) if !pat.is_empty() => format!("{}{}{}", &t[..i], s(&arg(&args, 1)), &t[i + pat.len()..]),
                _ => t,
            })
        }
        "remove" => Value::str(s(&input).replace(&s(&a0), "")),
        "remove_first" => Value::str(s(&input).replacen(&s(&a0), "", 1)),
        "remove_last" => {
            let t = s(&input);
            let pat = s(&a0);
            Value::str(match t.rfind(&pat) {
                Some(i) if !pat.is_empty() => format!("{}{}", &t[..i], &t[i + pat.len()..]),
                _ => t,
            })
        }
        "truncate" => {
            let t = s(&input);
            let n = if args.is_empty() { 50 } else { a0.as_int().unwrap_or(50).max(0) as usize };
            let ellipsis = if args.len() > 1 { s(&arg(&args, 1)) } else { "...".into() };
            let chars: Vec<char> = t.chars().collect();
            if chars.len() <= n {
                Value::str(t)
            } else {
                let keep = n.saturating_sub(ellipsis.chars().count());
                Value::str(chars[..keep].iter().collect::<String>() + &ellipsis)
            }
        }
        "truncatewords" => {
            let t = s(&input);
            let n = if args.is_empty() { 15 } else { a0.as_int().unwrap_or(15).max(1) as usize };
            let ellipsis = if args.len() > 1 { s(&arg(&args, 1)) } else { "...".into() };
            let words: Vec<&str> = t.split_whitespace().collect();
            if words.len() <= n { Value::str(t) } else { Value::str(words[..n].join(" ") + &ellipsis) }
        }
        "split" => {
            let t = s(&input);
            let sep = s(&a0);
            let parts: Vec<Value> = if sep.is_empty() {
                t.chars().map(|c| Value::str(c.to_string())).collect()
            } else if sep == " " {
                t.split_whitespace().map(Value::str).collect()
            } else {
                let mut v: Vec<&str> = t.split(&sep).collect();
                while v.last() == Some(&"") {
                    v.pop();
                }
                v.into_iter().map(Value::str).collect()
            };
            r.count("string_byte", t.len() as f64);
            Value::array(parts)
        }
        "slice" => {
            let start = a0.as_int().unwrap_or(0);
            let len = if args.len() > 1 { arg(&args, 1).as_int().unwrap_or(1) } else { 1 }.max(0) as usize;
            match &input {
                Value::Array(a) => {
                    let st = if start < 0 { (a.len() as i64 + start).max(0) } else { start } as usize;
                    Value::array(a.iter().skip(st).take(len).cloned().collect())
                }
                other => {
                    let chars: Vec<char> = s(other).chars().collect();
                    let st = if start < 0 { (chars.len() as i64 + start).max(0) } else { start } as usize;
                    Value::str(chars.iter().skip(st).take(len).collect::<String>())
                }
            }
        }
        "size" => Value::Int(input.len() as i64),
        "handleize" | "handle" => Value::str(handleize(&s(&input))),
        "camelize" => Value::str(s(&input).split(['-', '_', ' ']).map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }).collect::<String>()),
        "pluralize" => {
            let n = input.as_f64().unwrap_or(0.0);
            if n == 1.0 { a0 } else { arg(&args, 1) }
        }
        "base64_encode" | "base64_url_safe_encode" | "md5" | "sha1" | "sha256" | "hmac_sha1" | "hmac_sha256" => {
            Value::str(format!("{:x}", s(&input).len() * 2654435761))
        }
        "base64_decode" | "base64_url_safe_decode" => input,
        "highlight" | "highlight_active_tag" => input,
        "json" => {
            let mut o = String::new();
            to_json(&input, &mut o, 0);
            r.count("string_byte", o.len() as f64);
            Value::str(o)
        }

        // ---- math ----
        "plus" => num_op(&input, &a0, |a, b| a.checked_add(b), |a, b| a + b),
        "minus" => num_op(&input, &a0, |a, b| a.checked_sub(b), |a, b| a - b),
        "times" => num_op(&input, &a0, |a, b| a.checked_mul(b), |a, b| a * b),
        "divided_by" => num_op(&input, &a0, |a, b| if b == 0 { None } else { Some(a.div_euclid(b)) }, |a, b| a / b),
        "modulo" => num_op(&input, &a0, |a, b| if b == 0 { None } else { Some(a.rem_euclid(b)) }, |a, b| a % b),
        "abs" => match input.to_number() {
            Value::Int(i) => Value::Int(i.abs()),
            v => Value::Float(v.as_f64().unwrap_or(0.0).abs()),
        },
        "ceil" => Value::Int(input.to_number().as_f64().unwrap_or(0.0).ceil() as i64),
        "floor" => Value::Int(input.to_number().as_f64().unwrap_or(0.0).floor() as i64),
        "round" => {
            let x = input.to_number().as_f64().unwrap_or(0.0);
            let d = a0.as_int().unwrap_or(0);
            if d <= 0 {
                Value::Int(x.round() as i64)
            } else {
                let p = 10f64.powi(d as i32);
                Value::Float((x * p).round() / p)
            }
        }
        "at_least" => {
            let (x, y) = (input.to_number(), a0.to_number());
            if x.as_f64() < y.as_f64() { y } else { x }
        }
        "at_most" => {
            let (x, y) = (input.to_number(), a0.to_number());
            if x.as_f64() > y.as_f64() { y } else { x }
        }

        // ---- arrays ----
        "join" => {
            let sep = if args.is_empty() { " ".to_string() } else { s(&a0) };
            Value::str(input.to_items().iter().map(|v| v.render()).collect::<Vec<_>>().join(&sep))
        }
        "first" => match &input {
            Value::Str(t) => t.chars().next().map(|c| Value::str(c.to_string())).unwrap_or_default(),
            other => other.to_items().into_iter().next().unwrap_or_default(),
        },
        "last" => match &input {
            Value::Str(t) => t.chars().last().map(|c| Value::str(c.to_string())).unwrap_or_default(),
            other => other.to_items().pop().unwrap_or_default(),
        },
        "reverse" => {
            let mut v = input.to_items();
            v.reverse();
            Value::array(v)
        }
        "compact" => Value::array(input.to_items().into_iter().filter(|v| !v.is_nil()).collect()),
        "uniq" => {
            let mut out: Vec<Value> = Vec::new();
            for v in input.to_items() {
                if !out.iter().any(|x| x.liquid_eq(&v)) {
                    out.push(v);
                }
            }
            Value::array(out)
        }
        "concat" => {
            let mut v = input.to_items();
            v.extend(a0.to_items());
            Value::array(v)
        }
        "map" => {
            let prop = s(&a0);
            let items = input.to_items();
            let out = items.iter().map(|v| r.get_prop(v, &Value::str(&prop))).collect();
            Value::array(out)
        }
        "where" | "reject" => {
            let prop = s(&a0);
            let target = args.get(1).cloned();
            let mut out = Vec::new();
            for v in input.to_items() {
                let x = r.get_prop(&v, &Value::str(&prop));
                let hit = match &target {
                    Some(t) => x.liquid_eq(t),
                    None => x.is_truthy(),
                };
                if hit == (name == "where") {
                    out.push(v);
                }
            }
            Value::array(out)
        }
        "find" | "find_index" | "has" => {
            let prop = s(&a0);
            let target = args.get(1).cloned();
            let items = input.to_items();
            let mut found = None;
            for (i, v) in items.iter().enumerate() {
                let x = r.get_prop(v, &Value::str(&prop));
                let hit = match &target {
                    Some(t) => x.liquid_eq(t),
                    None => x.is_truthy(),
                };
                if hit {
                    found = Some((i, v.clone()));
                    break;
                }
            }
            match name {
                "find" => found.map(|f| f.1).unwrap_or_default(),
                "find_index" => found.map(|f| Value::Int(f.0 as i64)).unwrap_or_default(),
                _ => Value::Bool(found.is_some()),
            }
        }
        "sort" | "sort_natural" => {
            let prop = if args.is_empty() { None } else { Some(s(&a0)) };
            let items = input.to_items();
            let mut keyed: Vec<(Value, Value)> =
                items.into_iter().map(|v| (sort_key(&v, prop.as_deref(), r), v)).collect();
            let natural = name == "sort_natural";
            keyed.sort_by(|a, b| {
                if natural {
                    a.0.render().to_lowercase().cmp(&b.0.render().to_lowercase())
                } else {
                    a.0.compare(&b.0).unwrap_or(std::cmp::Ordering::Equal)
                }
            });
            Value::array(keyed.into_iter().map(|k| k.1).collect())
        }
        "sum" => {
            let prop = if args.is_empty() { None } else { Some(s(&a0)) };
            let mut total = 0.0;
            let mut float = false;
            for v in input.to_items() {
                let x = sort_key(&v, prop.as_deref(), r).to_number();
                float |= matches!(x, Value::Float(_));
                total += x.as_f64().unwrap_or(0.0);
            }
            if float { Value::Float(total) } else { Value::Int(total as i64) }
        }
        "default" => {
            let allow_false = kwarg(&kwargs, "allow_false").is_some_and(Value::is_truthy);
            let empty = input.is_nil()
                || (matches!(input, Value::Bool(false)) && !allow_false)
                || matches!(&input, Value::Str(t) if t.is_empty())
                || matches!(&input, Value::Array(a) if a.is_empty());
            if empty { a0 } else { input }
        }
        "date" => {
            let fmt = s(&a0);
            if input.is_nil() { Value::Nil } else if fmt.is_empty() { input } else { Value::str("Nov 13, 2026") }
        }

        // ---- Shopify: money ----
        "money" => money(&input, false, true),
        "money_with_currency" => money(&input, true, true),
        "money_without_currency" => Value::str(format!("{:.2}", input.as_f64().unwrap_or(0.0) / 100.0)),
        "money_without_trailing_zeros" => money(&input, false, false),

        // ---- Shopify: translations ----
        "t" | "translate" => translate(r, &s(&input), &kwargs),
        "format_address" => Value::str("<p>1 Main St<br>Toronto ON<br>Canada</p>"),

        // ---- Shopify: URLs and HTML ----
        "image_url" | "img_url" | "product_img_url" | "collection_img_url" | "article_img_url" => {
            let mut url = image_src(&input);
            if name == "image_url" {
                url = with_params(&url, &kwargs);
            } else if !a0.is_nil() {
                url = url.replace(".jpg", &format!("_{}.jpg", s(&a0)));
            }
            r.count("string_byte", url.len() as f64);
            Value::str(url)
        }
        "image_tag" | "img_tag" => {
            let src = image_src(&input);
            let mut attrs = String::new();
            for (k, v) in &kwargs {
                if k != "widths" && k != "preload" {
                    attrs.push_str(&format!(" {k}=\"{}\"", v.render()));
                }
            }
            let widths: Vec<String> = match kwarg(&kwargs, "widths") {
                Some(w) => w.render().split(',').map(|x| x.trim().to_string()).collect(),
                None => vec!["352".into(), "832".into(), "1200".into()],
            };
            let srcset: Vec<String> = widths.iter().map(|w| format!("{src}&width={w} {w}w")).collect();
            let html = format!(
                "<img src=\"{src}\" alt=\"\" srcset=\"{}\" width=\"1200\" height=\"1200\"{attrs}>",
                srcset.join(", ")
            );
            r.count("string_byte", html.len() as f64);
            Value::str(html)
        }
        "asset_url" | "asset_img_url" | "file_url" | "file_img_url" | "shopify_asset_url" | "global_asset_url" => {
            Value::str(format!("//cdn.shopify.com/s/files/1/0000/0001/t/1/assets/{}?v=1700000000", s(&input)))
        }
        "stylesheet_tag" => Value::str(format!("<link href=\"{}\" rel=\"stylesheet\" type=\"text/css\" media=\"all\" />", s(&input))),
        "script_tag" => Value::str(format!("<script src=\"{}\" type=\"text/javascript\"></script>", s(&input))),
        "preload_tag" => Value::str(format!("<link href=\"{}\" rel=\"preload\" as=\"style\">", s(&input))),
        "link_to" | "link_to_vendor" | "link_to_type" | "link_to_tag" | "link_to_add_tag" | "link_to_remove_tag" => {
            Value::str(format!("<a href=\"{}\">{}</a>", s(&a0), s(&input)))
        }
        "url_for_vendor" | "url_for_type" => Value::str(format!("/collections/types?q={}", handleize(&s(&input)))),
        "within" => input,
        "sort_by" => Value::str(format!("{}?sort_by={}", s(&input), s(&a0))),
        "customer_login_link" | "customer_logout_link" | "customer_register_link" => {
            Value::str(format!("<a href=\"/account/login\">{}</a>", s(&input)))
        }
        "default_pagination" => Value::str("<span class=\"page current\">1</span> <span class=\"page\"><a href=\"?page=2\">2</a></span>"),
        "placeholder_svg_tag" => Value::str(format!(
            "<svg class=\"{}\" xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 525.5 525.5\"><path d=\"{}\"/></svg>",
            s(&a0),
            "M".repeat(600)
        )),
        "payment_type_svg_tag" => Value::str(format!("<svg viewBox=\"0 0 38 24\"><path d=\"{}\"/></svg>", "M".repeat(400))),
        "payment_type_img_url" => Value::str("//cdn.shopify.com/s/assets/payment_icons/visa.svg"),
        "inline_asset_content" => Value::str(format!("<svg>{}</svg>", "<path d=\"M0 0\"/>".repeat(20))),
        "structured_data" => Value::str(format!("<script type=\"application/ld+json\">{}</script>", "{\"@type\":\"Product\"}".repeat(10))),
        "metafield_tag" | "metafield_text" => {
            let v = match &input {
                Value::Object(o) => o.get("value").cloned().unwrap_or_default(),
                other => other.clone(),
            };
            Value::str(if name == "metafield_tag" { format!("<span class=\"metafield\">{}</span>", v.render()) } else { v.render() })
        }
        "time_tag" => Value::str(format!("<time datetime=\"2026-11-13T00:00:00Z\">{}</time>", s(&input))),
        "weight_with_unit" => Value::str(format!("{} kg", input.as_f64().unwrap_or(0.0) / 1000.0)),
        "external_video_tag" | "video_tag" | "media_tag" | "model_viewer_tag" => {
            Value::str(format!("<video playsinline controls><source src=\"{}\"></video>", image_src(&input)))
        }
        "external_video_url" => Value::str("https://www.youtube.com/embed/abc"),
        "font_face" => Value::str(format!(
            "@font-face {{ font-family: Assistant; font-weight: 400; font-style: normal; font-display: {}; src: url(//cdn.shopify.com/fonts/assistant_n4.woff2) format(\"woff2\"); }}",
            kwarg(&kwargs, "font_display").map(|v| v.render()).unwrap_or_else(|| "swap".into())
        )),
        "font_url" => Value::str("//cdn.shopify.com/fonts/assistant_n4.woff2"),
        "font_modify" => input,
        "color_to_rgb" | "color_to_hsl" | "color_to_hex" | "color_extract" | "color_brightness" | "color_modify"
        | "color_lighten" | "color_darken" | "color_saturate" | "color_desaturate" | "color_mix"
        | "color_contrast" | "color_difference" | "brightness_difference" | "hex_to_rgba" => {
            if name == "color_brightness" || name == "color_contrast" || name == "color_difference" || name == "brightness_difference" {
                Value::Float(128.5)
            } else {
                Value::str("rgba(18, 18, 18, 1.0)")
            }
        }
        "money_amount" => Value::str(format!("{:.2}", input.as_f64().unwrap_or(0.0) / 100.0)),
        "standard_event_data" => Value::str("{\"productVariant\":{\"id\":\"1\"}}"),
        "payment_button" => Value::str("<div data-shopify=\"payment-button\" class=\"shopify-payment-button\"><shopify-accelerated-checkout></shopify-accelerated-checkout></div>"),
        "payment_terms" => Value::str("<shopify-payment-terms variant-id=\"1\"></shopify-payment-terms>"),
        "applicable_disclosures" => Value::array(Vec::new()),
        "item_count_for_variant" => Value::Int(0),
        "line_items_for" => Value::array(Vec::new()),
        "class_list" => Value::str("color-scheme-1 gradient"),
        "currency_selector" | "login_button" | "shopify_button" => Value::str(""),
        "avatar" => Value::str("<img class=\"avatar\">"),
        "app_block_class" => Value::str("app-block"),
        // Unknown filters return their input, as the renderer would after logging an error.
        _ => {
            r.unsupported.insert(format!("filter {name}"));
            input
        }
    }
}

/// The `t` filter: a dotted key into the default locale, with `{{ var }}` interpolation and
/// `count` pluralization.
fn translate(r: &mut Renderer<'_>, key: &str, kwargs: &[(String, Value)]) -> Value {
    let mut v = r.translations.clone();
    for part in key.split('.') {
        v = match &v {
            Value::Object(o) => o.get(part).cloned().unwrap_or_default(),
            _ => Value::Nil,
        };
    }
    if let (Value::Object(o), Some(count)) = (&v, kwarg(kwargs, "count")) {
        let n = count.as_f64().unwrap_or(0.0);
        let form = if n == 0.0 && o.get("zero").is_some() { "zero" } else if n == 1.0 { "one" } else { "other" };
        v = o.get(form).or_else(|| o.get("other")).cloned().unwrap_or_default();
    }
    let Value::Str(text) = v else {
        return Value::str(format!("translation missing: en.{key}"));
    };
    if !text.contains("{{") {
        return Value::Str(text);
    }
    let mut out = String::new();
    let mut rest: &str = &text;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find("}}") else { break };
        let name = rest[i + 2..i + j].trim();
        if let Some(val) = kwarg(kwargs, name) {
            val.render_into(&mut out);
        }
        rest = &rest[i + j + 2..];
    }
    out.push_str(rest);
    Value::str(out)
}
