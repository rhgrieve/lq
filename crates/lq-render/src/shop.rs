//! The mock shop: generated catalogue data with the shapes Shopify's Liquid objects have.
//!
//! `generate` writes a JSON document (`.lq/shop.json`) that can be edited by hand; `Shop::load`
//! reads it back. Objects carry `__type` so the cost model can price database-backed reads.
//! Shared objects are written once and referenced as `{"__ref": "product:<handle>"}`.

use crate::value::{Object, Value};
use serde_json::{json, Map, Value as J};
use std::collections::HashMap;
use std::rc::Rc;

/// Sizes of the generated catalogue. Defaults describe a mid-size store.
#[derive(Clone, Debug)]
pub struct Knobs {
    pub products: usize,
    pub options: usize,
    pub values_per_option: usize,
    /// Variants per product; 0 for every combination of option values.
    pub variants_per_product: usize,
    pub images_per_product: usize,
    pub collections: usize,
    pub collection_size: usize,
    pub metafields_per_product: usize,
    pub tags_per_product: usize,
    pub cart_items: usize,
    pub articles: usize,
    pub countries: usize,
    pub languages: usize,
    pub menu_links: usize,
    pub menu_children: usize,
    pub filters: usize,
    pub logged_in: bool,
}

impl Default for Knobs {
    fn default() -> Self {
        Knobs {
            products: 48,
            options: 2,
            values_per_option: 3,
            variants_per_product: 0,
            images_per_product: 4,
            collections: 6,
            collection_size: 24,
            metafields_per_product: 4,
            tags_per_product: 5,
            cart_items: 3,
            articles: 6,
            countries: 20,
            languages: 2,
            menu_links: 6,
            menu_children: 4,
            filters: 3,
            logged_in: false,
        }
    }
}

macro_rules! knob_fields {
    ($m:ident, $($f:ident),*) => {
        $( if let Some(v) = $m.get(stringify!($f)).and_then(J::as_u64) { $f = v as usize; } )*
    };
}

impl Knobs {
    pub fn to_json(&self) -> J {
        json!({
            "products": self.products, "options": self.options,
            "values_per_option": self.values_per_option, "variants_per_product": self.variants_per_product,
            "images_per_product": self.images_per_product,
            "collections": self.collections, "collection_size": self.collection_size,
            "metafields_per_product": self.metafields_per_product, "tags_per_product": self.tags_per_product,
            "cart_items": self.cart_items, "articles": self.articles, "countries": self.countries,
            "languages": self.languages, "menu_links": self.menu_links, "menu_children": self.menu_children,
            "filters": self.filters, "logged_in": self.logged_in,
        })
    }

    pub fn from_json(m: &J) -> Knobs {
        let d = Knobs::default();
        let Knobs {
            mut products, mut options, mut values_per_option, mut variants_per_product, mut images_per_product, mut collections,
            mut collection_size, mut metafields_per_product, mut tags_per_product, mut cart_items,
            mut articles, mut countries, mut languages, mut menu_links, mut menu_children, mut filters, ..
        } = d;
        knob_fields!(m, products, options, values_per_option, variants_per_product, images_per_product, collections,
            collection_size, metafields_per_product, tags_per_product, cart_items, articles, countries,
            languages, menu_links, menu_children, filters);
        Knobs {
            products, options, values_per_option, variants_per_product, images_per_product, collections, collection_size,
            metafields_per_product, tags_per_product, cart_items, articles, countries, languages,
            menu_links, menu_children, filters,
            logged_in: m.get("logged_in").and_then(J::as_bool).unwrap_or(false),
        }
    }

    /// Applies `name=value` overrides from the command line.
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), String> {
        let mut j = self.to_json();
        let obj = j.as_object_mut().unwrap();
        if !obj.contains_key(name) {
            return Err(format!("unknown shop setting '{name}'"));
        }
        let v = if name == "logged_in" {
            J::Bool(matches!(value, "true" | "1" | "yes"))
        } else {
            J::from(value.parse::<u64>().map_err(|_| format!("{name} needs a number"))?)
        };
        obj.insert(name.into(), v);
        *self = Knobs::from_json(&j);
        Ok(())
    }
}

const WORDS: &[&str] = &[
    "Alpine", "Arctic", "Boreal", "Cedar", "Coastal", "Drift", "Ember", "Field", "Glacier", "Harbor",
    "Juniper", "Lumen", "Meadow", "Nomad", "Ridge", "Summit", "Tundra", "Willow",
];
const KINDS: &[&str] = &["Parka", "Jacket", "Vest", "Hoodie", "Beanie", "Gloves", "Scarf", "Boots"];
/// Shopify's limit on variants per product.
const MAX_VARIANTS: usize = 2000;

const OPTIONS: &[(&str, &[&str])] = &[
    ("Size", &["XS", "S", "M", "L", "XL", "XXL"]),
    ("Color", &["Black", "Navy", "Red", "Green", "White", "Grey"]),
    ("Material", &["Down", "Wool", "Cotton"]),
];

fn image(id: usize, handle: &str, n: usize) -> J {
    let src = format!("//cdn.shopify.com/s/files/1/0000/0001/products/{handle}-{n}.jpg?v=1700000000");
    json!({
        "__type": "image", "id": id, "src": src, "alt": format!("{handle} image {n}"),
        "width": 1600, "height": 2000, "aspect_ratio": 0.8, "position": n, "media_type": "image",
        "attached_to_variant?": false, "presentation": {"focal_point": "50.0% 50.0%"},
        "preview_image": {"__type": "image", "src": src, "width": 1600, "height": 2000, "aspect_ratio": 0.8, "alt": ""},
        "__to_s": src,
    })
}

fn lorem(words: usize, seed: usize) -> String {
    (0..words).map(|i| WORDS[(i * 7 + seed) % WORDS.len()].to_lowercase()).collect::<Vec<_>>().join(" ")
}

fn product(i: usize, k: &Knobs) -> J {
    let title = format!("{} {}", WORDS[i % WORDS.len()], KINDS[(i / WORDS.len() + i) % KINDS.len()]);
    let handle = format!("{}-{}", title.to_lowercase().replace(' ', "-"), i + 1);
    let base = 19_900 + (i as i64 % 9) * 10_000;
    let images: Vec<J> = (1..=k.images_per_product.max(1)).map(|n| image(1000 * (i + 1) + n, &handle, n)).collect();
    let opts: Vec<(&str, Vec<String>)> = OPTIONS
        .iter()
        .take(k.options)
        .map(|(name, vals)| {
            // Past the named values, number them: "Size 7", "Size 8", ...
            let values = (0..k.values_per_option.max(1))
                .map(|n| vals.get(n).map_or_else(|| format!("{name} {}", n + 1), |v| v.to_string()))
                .collect();
            (*name, values)
        })
        .collect();
    // Every combination of option values is a variant.
    let mut combos: Vec<Vec<&str>> = vec![vec![]];
    for (_, vals) in &opts {
        combos = combos.into_iter().flat_map(|c| vals.iter().map(move |v| { let mut c = c.clone(); c.push(v.as_str()); c })).collect();
    }
    // Not every combination exists on real products: keep an even spread of them.
    let want = if k.variants_per_product == 0 { combos.len() } else { k.variants_per_product.min(combos.len()) };
    let want = want.min(MAX_VARIANTS);
    if want < combos.len() {
        let step = combos.len() as f64 / want as f64;
        combos = (0..want).map(|n| combos[(n as f64 * step) as usize].clone()).collect();
    }
    let variants: Vec<J> = combos
        .iter()
        .enumerate()
        .map(|(vi, combo)| {
            let id = 400_000_000 + i * 10_000 + vi;
            let title = if combo.is_empty() { "Default Title".to_string() } else { combo.join(" / ") };
            let price = base + (vi as i64 % 3) * 1000;
            json!({
                "__type": "variant", "id": id, "title": title, "name": format!("{} - {title}", "p"),
                "option1": combo.first(), "option2": combo.get(1), "option3": combo.get(2),
                "options": combo, "price": price, "compare_at_price": if i.is_multiple_of(4) { J::from(price + 5000) } else { J::Null },
                "available": vi % 5 != 4, "sku": format!("SKU-{id}"), "barcode": J::Null,
                "featured_image": images[vi % images.len()], "featured_media": images[vi % images.len()],
                "image": images[vi % images.len()], "url": format!("/products/{handle}?variant={id}"),
                "inventory_quantity": 10 + vi, "inventory_management": "shopify", "inventory_policy": "deny",
                "requires_shipping": true, "requires_selling_plan": false, "weight": 1200, "weight_unit": "kg",
                "unit_price": J::Null, "unit_price_measurement": J::Null, "selected": vi == 0, "matched": true,
                "metafields": {"__type": "metafields"}, "store_availabilities": [], "selling_plan_allocations": [],
                "quantity_rule": {"min": 1, "max": J::Null, "increment": 1}, "quantity_price_breaks": [],
                "taxable": true, "__to_s": id.to_string(),
            })
        })
        .collect();
    let options_with_values: Vec<J> = opts
        .iter()
        .enumerate()
        .map(|(oi, (name, vals))| {
            let values: Vec<J> = vals
                .iter()
                .enumerate()
                .map(|(vi, v)| json!({
                    "__type": "product_option_value", "name": v, "id": oi * 10 + vi, "selected": vi == 0,
                    "available": true, "swatch": J::Null, "variant": J::Null, "product_url": J::Null,
                    "__to_s": v,
                }))
                .collect();
            json!({
                "__type": "product_option", "name": name, "position": oi + 1, "values": values,
                "selected_value": vals.first(), "__to_s": name,
            })
        })
        .collect();
    let metafields: Map<String, J> = (0..k.metafields_per_product)
        .map(|m| {
            (format!("field_{m}"), json!({"__type": "metafield", "value": lorem(6, i + m), "type": "single_line_text_field", "__to_s": lorem(6, i + m)}))
        })
        .collect();
    let mut metafields = Map::from_iter([("custom".to_string(), J::Object(metafields))]);
    metafields.insert("__type".into(), J::from("metafields"));
    let tags: Vec<String> = (0..k.tags_per_product).map(|t| WORDS[(i + t * 3) % WORDS.len()].to_lowercase()).collect();
    json!({
        "__type": "product", "id": 8_000_000 + i, "handle": handle, "title": title,
        "vendor": "Northbound", "type": KINDS[i % KINDS.len()], "url": format!("/products/{handle}"),
        "description": format!("<p>{}</p><ul><li>{}</li><li>{}</li></ul>", lorem(60, i), lorem(8, i + 1), lorem(8, i + 2)),
        "content": format!("<p>{}</p>", lorem(60, i)),
        "price": base, "price_min": base, "price_max": base + 2000, "price_varies": variants.len() > 1,
        "compare_at_price": if i.is_multiple_of(4) { J::from(base + 5000) } else { J::Null },
        "compare_at_price_min": base, "compare_at_price_max": base + 5000, "compare_at_price_varies": false,
        "available": true, "featured_image": images[0], "featured_media": images[0],
        "images": images, "media": images, "variants": variants,
        "options": opts.iter().map(|o| o.0).collect::<Vec<_>>(),
        "options_by_name": J::Object(options_with_values.iter().map(|o| (o["name"].as_str().unwrap_or("").to_string(), o.clone())).collect()),
        "options_with_values": options_with_values,
        "has_only_default_variant": opts.is_empty(), "tags": tags, "metafields": metafields,
        "selling_plan_groups": [], "requires_selling_plan": false, "gift_card?": false,
        "quantity_price_breaks_configured?": false, "template_suffix": J::Null, "selected_variant": J::Null,
        "created_at": "2026-01-01T00:00:00Z", "published_at": "2026-01-01T00:00:00Z",
        "collections": [], "__to_s": title,
    })
}

fn link(title: &str, url: &str, children: Vec<J>) -> J {
    json!({
        "__type": "link", "title": title, "url": url, "handle": title.to_lowercase().replace(' ', "-"),
        "active": false, "child_active": false, "current": false, "child_current": false,
        "levels": if children.is_empty() { 0 } else { 1 }, "type": "collection_link", "object": J::Null,
        "links": children, "__to_s": title,
    })
}

/// Generates a mock shop document.
pub fn generate(k: &Knobs) -> J {
    let products: Vec<J> = (0..k.products).map(|i| product(i, k)).collect();
    let handles: Vec<String> = products.iter().map(|p| p["handle"].as_str().unwrap().to_string()).collect();
    let pref = |h: &str| json!({"__ref": format!("product:{h}")});
    let filters: Vec<J> = (0..k.filters)
        .map(|f| {
            if f == k.filters - 1 {
                json!({"__type": "filter", "label": "Price", "param_name": "filter.v.price", "type": "price_range",
                    "min_value": {"value": J::Null, "param_name": "filter.v.price.gte"},
                    "max_value": {"value": J::Null, "param_name": "filter.v.price.lte"},
                    "range_max": 59_900, "values": [], "active_values": [], "presentation": J::Null,
                    "url_to_remove": "/collections/all"})
            } else {
                let (name, vals) = OPTIONS[f % OPTIONS.len()];
                let values: Vec<J> = vals.iter().take(5).map(|v| json!({
                    "__type": "filter_value", "label": v, "value": v, "param_name": format!("filter.v.option.{}", name.to_lowercase()),
                    "count": 12, "active": false, "url_to_add": format!("?filter.v.option={v}"), "url_to_remove": "?",
                    "swatch": J::Null, "image": J::Null,
                })).collect();
                json!({"__type": "filter", "label": name, "param_name": format!("filter.v.option.{}", name.to_lowercase()),
                    "type": "list", "values": values, "active_values": [], "inactive_values": [], "presentation": "text",
                    "operator": "OR", "url_to_remove": "/collections/all"})
            }
        })
        .collect();
    let collections: Vec<J> = (0..k.collections)
        .map(|c| {
            let handle = if c == 0 { "all".to_string() } else { format!("{}-{}", KINDS[c % KINDS.len()].to_lowercase(), c) };
            let members: Vec<J> = (0..k.collection_size.min(k.products)).map(|j| pref(&handles[(c * 5 + j) % handles.len()])).collect();
            json!({
                "__type": "collection", "id": 4_000_000 + c, "handle": handle,
                "title": if c == 0 { "All".to_string() } else { format!("{}s", KINDS[c % KINDS.len()]) },
                "description": format!("<p>{}</p>", lorem(20, c)), "url": format!("/collections/{handle}"),
                "image": image(9000 + c, &handle, 1), "featured_image": image(9000 + c, &handle, 1),
                "products": members, "products_count": k.collection_size, "all_products_count": k.collection_size,
                "all_tags": ["arctic", "boreal"], "all_types": KINDS, "all_vendors": ["Northbound"],
                "filters": filters, "sort_by": "manual", "default_sort_by": "manual",
                "sort_options": [{"name": "Featured", "value": "manual"}, {"name": "Price, low to high", "value": "price-ascending"}],
                "current_type": J::Null, "current_vendor": J::Null, "template_suffix": J::Null,
                "metafields": {"__type": "metafields"}, "__count": k.collection_size, "__to_s": handle,
            })
        })
        .collect();
    let menu = |handle: &str, n: usize, depth: usize| {
        let links: Vec<J> = (0..n)
            .map(|l| {
                let kids = if depth > 0 {
                    (0..k.menu_children).map(|c| link(WORDS[(l + c) % WORDS.len()], "/collections/all", vec![])).collect()
                } else {
                    vec![]
                };
                link(KINDS[l % KINDS.len()], &format!("/collections/{}", KINDS[l % KINDS.len()].to_lowercase()), kids)
            })
            .collect();
        json!({"__type": "linklist", "handle": handle, "title": handle, "levels": depth + 1, "links": links, "__to_s": handle})
    };
    let articles: Vec<J> = (0..k.articles)
        .map(|a| json!({
            "__type": "article", "id": 6_000_000 + a, "handle": format!("story-{a}"), "title": format!("{} story", WORDS[a % WORDS.len()]),
            "author": "Editor", "content": format!("<p>{}</p>", lorem(200, a)), "excerpt": format!("<p>{}</p>", lorem(30, a)),
            "excerpt_or_content": format!("<p>{}</p>", lorem(30, a)), "image": image(7000 + a, "story", a),
            "url": format!("/blogs/news/story-{a}"), "published_at": "2026-10-01T00:00:00Z", "created_at": "2026-10-01T00:00:00Z",
            "tags": ["news"], "comments_count": 0, "comments": [], "comments_enabled?": false,
            "user": {"first_name": "Ed", "last_name": "Itor", "name": "Ed Itor", "image": J::Null},
            "metafields": {"__type": "metafields"}, "__to_s": format!("story-{a}"),
        }))
        .collect();
    let blog = json!({
        "__type": "blog", "id": 5_000_000, "handle": "news", "title": "News", "url": "/blogs/news",
        "articles": articles.iter().map(|a| json!({"__ref": format!("article:{}", a["handle"].as_str().unwrap())})).collect::<Vec<_>>(),
        "articles_count": k.articles, "all_tags": ["news"], "tags": ["news"], "comments_enabled?": false,
        "metafields": {"__type": "metafields"}, "__to_s": "news",
    });
    let items: Vec<J> = (0..k.cart_items.min(k.products))
        .map(|n| {
            let p = &products[n * 3 % products.len()];
            let v = &p["variants"][0];
            let price = v["price"].as_i64().unwrap_or(0);
            json!({
                "__type": "line_item", "id": v["id"], "key": format!("{}:abc{n}", v["id"]), "product": pref(p["handle"].as_str().unwrap()),
                "variant": v, "title": p["title"], "product_title": p["title"], "variant_title": v["title"],
                "quantity": 1 + n % 2, "price": price, "final_price": price, "original_price": price,
                "line_price": price, "final_line_price": price, "original_line_price": price,
                "url": v["url"], "image": p["featured_image"], "featured_image": p["featured_image"],
                "options_with_values": [], "properties": {}, "selling_plan_allocation": J::Null, "discounts": [],
                "line_level_discount_allocations": [], "unit_price_measurement": J::Null, "sku": v["sku"],
                "vendor": p["vendor"], "variant_id": v["id"], "product_id": p["id"], "has_components?": false,
                "gift_card": false, "requires_shipping": true,
            })
        })
        .collect();
    let total: i64 = items.iter().map(|i| i["final_line_price"].as_i64().unwrap_or(0)).sum();
    let currency = |iso: &str, sym: &str| json!({"__type": "currency", "iso_code": iso, "symbol": sym, "name": iso, "__to_s": iso});
    let countries: Vec<J> = (0..k.countries)
        .map(|c| json!({
            "__type": "country", "iso_code": format!("C{c}"), "name": format!("Country {c}"), "unit_system": "metric",
            "currency": currency(if c == 0 { "CAD" } else { "USD" }, "$"), "popular?": c < 3,
            "available_languages": [], "market": {"handle": "intl"}, "continent": "North America", "__to_s": format!("C{c}"),
        }))
        .collect();
    let languages: Vec<J> = (0..k.languages)
        .map(|l| json!({"__type": "shop_locale", "iso_code": if l == 0 { "en".to_string() } else { format!("l{l}") },
            "name": "English", "endonym_name": "English", "primary": l == 0, "root_url": "/", "__to_s": "en"}))
        .collect();
    let customer = if k.logged_in {
        json!({"__type": "customer", "id": 1, "first_name": "Sam", "last_name": "Lee", "name": "Sam Lee", "email": "sam@example.com",
            "orders": [], "orders_count": 0, "addresses": [], "default_address": J::Null, "tags": [], "has_account": true,
            "metafields": {"__type": "metafields"}})
    } else {
        J::Null
    };
    json!({
        "version": 1,
        "knobs": k.to_json(),
        "products": products,
        "collections": collections,
        "articles": articles,
        "blogs": [blog],
        "pages": [
            {"__type": "page", "id": 1, "handle": "about-us", "title": "About us", "url": "/pages/about-us", "content": format!("<p>{}</p>", lorem(150, 3)), "template_suffix": J::Null, "metafields": {"__type": "metafields"}, "__to_s": "about-us"},
            {"__type": "page", "id": 2, "handle": "contact", "title": "Contact", "url": "/pages/contact", "content": "<p>Get in touch.</p>", "template_suffix": "contact", "metafields": {"__type": "metafields"}, "__to_s": "contact"},
        ],
        "linklists": [menu("main-menu", k.menu_links, 1), menu("footer", k.menu_links, 0), menu("customer-account-main-menu", 2, 0)],
        "shop": {
            "__type": "shop", "id": 1, "name": "Northbound", "url": "https://northbound.example", "domain": "northbound.example",
            "secure_url": "https://northbound.example", "permanent_domain": "northbound.myshopify.com", "email": "hello@example.com",
            "currency": "USD", "money_format": "${{amount}}", "money_with_currency_format": "${{amount}} USD",
            "description": "Outerwear.", "locale": "en", "enabled_payment_types": ["visa", "master", "american_express", "paypal", "shopify_pay"],
            "customer_accounts_enabled": true, "customer_accounts_optional": true, "products_count": k.products,
            "collections_count": k.collections, "types": KINDS, "vendors": ["Northbound"], "policies": [],
            "brand": {"logo": J::Null, "square_logo": J::Null, "short_description": "Outerwear", "colors": {}},
            "published_locales": languages, "metafields": {"__type": "metafields"}, "accepts_gift_cards": true,
            "__to_s": "Northbound",
        },
        "cart": {
            "__type": "cart", "item_count": items.iter().map(|i| i["quantity"].as_i64().unwrap_or(1)).sum::<i64>(),
            "items": items, "total_price": total, "original_total_price": total, "items_subtotal_price": total,
            "total_discount": 0, "note": J::Null, "attributes": {}, "cart_level_discount_applications": [],
            "discount_applications": [], "currency": currency("USD", "$"), "requires_shipping": true,
            "empty?": k.cart_items == 0, "checkout_charge_amount": total, "duties_included": false, "taxes_included": false,
        },
        "customer": customer,
        "localization": {
            "__type": "localization", "available_countries": countries, "available_languages": languages,
            "country": countries.first(), "language": languages.first(),
            "market": {"__type": "market", "id": 1, "handle": "primary", "metafields": {"__type": "metafields"}},
        },
    })
}

/// A loaded mock shop: shared objects by kind and handle, plus top-level globals.
pub struct Shop {
    pub objects: HashMap<String, Value>,
    pub products: Vec<Value>,
    pub collections: Vec<Value>,
    pub articles: Vec<Value>,
    pub blogs: Vec<Value>,
    pub pages: Vec<Value>,
    pub linklists: Vec<Value>,
    pub globals: HashMap<String, Value>,
    pub knobs: Knobs,
}

impl Shop {
    pub fn load(text: &str) -> Result<Shop, String> {
        let j: J = serde_json::from_str(text).map_err(|e| format!("shop file: {e}"))?;
        Ok(Shop::from_json(&j))
    }

    pub fn from_json(j: &J) -> Shop {
        let mut objects = HashMap::new();
        let take = |kind: &str, objects: &mut HashMap<String, Value>| -> Vec<Value> {
            let list: Vec<Value> = j[kind].as_array().map(|a| a.iter().map(|x| convert(x, objects)).collect()).unwrap_or_default();
            let singular = match kind {
                "products" => "product",
                "collections" => "collection",
                "articles" => "article",
                "blogs" => "blog",
                "pages" => "page",
                _ => "linklist",
            };
            for v in &list {
                if let Value::Object(o) = v {
                    let handle = o.get("handle").map(|h| h.render()).unwrap_or_default();
                    objects.insert(format!("{singular}:{handle}"), v.clone());
                }
            }
            list
        };
        // Order matters: later kinds reference earlier ones.
        let products = take("products", &mut objects);
        let collections = take("collections", &mut objects);
        let articles = take("articles", &mut objects);
        let blogs = take("blogs", &mut objects);
        let pages = take("pages", &mut objects);
        let linklists = take("linklists", &mut objects);
        let mut globals = HashMap::new();
        for g in ["shop", "cart", "customer", "localization"] {
            globals.insert(g.to_string(), convert(&j[g], &objects));
        }
        let knobs = Knobs::from_json(&j["knobs"]);
        Shop { objects, products, collections, articles, blogs, pages, linklists, globals, knobs }
    }
}

/// JSON to a Liquid value, resolving `__ref`s and filling in the derived product fields.
fn convert(j: &J, objects: &HashMap<String, Value>) -> Value {
    match j {
        J::Null => Value::Nil,
        J::Bool(b) => Value::Bool(*b),
        J::Number(n) => n.as_i64().map(Value::Int).unwrap_or_else(|| Value::Float(n.as_f64().unwrap_or(0.0))),
        J::String(s) => Value::str(s),
        J::Array(a) => Value::array(a.iter().map(|x| convert(x, objects)).collect()),
        J::Object(m) => {
            if let Some(r) = m.get("__ref").and_then(J::as_str) {
                return objects.get(r).cloned().unwrap_or_default();
            }
            let ty = m.get("__type").and_then(J::as_str).unwrap_or("");
            let mut o = Object::new(ty);
            for (k, v) in m {
                if k != "__type" {
                    o.set(k, convert(v, objects));
                }
            }
            if ty == "product" {
                let first = match o.get("variants") {
                    Some(Value::Array(v)) => v.iter().find(|x| matches!(x, Value::Object(x) if x.get("available").is_some_and(Value::is_truthy))).or(v.first()).cloned(),
                    _ => None,
                };
                if let Some(first) = first {
                    o.set("selected_or_first_available_variant", first.clone());
                    o.set("first_available_variant", first);
                }
            }
            Value::Object(Rc::new(o))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_and_loads() {
        let k = Knobs { products: 5, collections: 2, collection_size: 3, ..Knobs::default() };
        let j = generate(&k);
        let shop = Shop::load(&j.to_string()).unwrap();
        assert_eq!(shop.products.len(), 5);
        let Value::Object(c) = &shop.collections[1] else { panic!() };
        let Some(Value::Array(ps)) = c.get("products") else { panic!() };
        assert_eq!(ps.len(), 3);
        // References share the product object.
        let Value::Object(p) = &ps[0] else { panic!() };
        assert!(shop.products.iter().any(|x| matches!(x, Value::Object(o) if o.id == p.id)));
        assert!(p.get("selected_or_first_available_variant").is_some());
    }
}
