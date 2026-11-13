use lq_render::{render_page, CostModel, Knobs, Limits, PageRequest, Shop, Theme};
use std::collections::HashMap;

fn shop() -> Shop {
    let k = Knobs { products: 4, collections: 2, collection_size: 3, ..Knobs::default() };
    Shop::from_json(&lq_render::shop::generate(&k))
}

fn render_with(files: &[(&str, &str)], template: &str) -> lq_render::PageResult {
    let mut all: Vec<(String, String)> = vec![("layout/theme.liquid".into(), "{{ content_for_layout }}".into())];
    all.extend(files.iter().map(|(p, s)| (p.to_string(), s.to_string())));
    let theme = Theme::from_files(all);
    let model = CostModel::builtin();
    let req = PageRequest { template: template.into(), handle: None };
    render_page(&theme, &shop(), &req, &model, Limits::default(), &HashMap::new()).unwrap()
}

fn render(src: &str) -> String {
    render_with(&[("templates/index.liquid", src)], "index").html
}

#[test]
fn output_and_filters() {
    assert_eq!(render("{{ 'a' | append: 'b' | upcase }}"), "AB");
    assert_eq!(render("{{ 10 | divided_by: 3 }} {{ 10.0 | divided_by: 4 }}"), "3 2.5");
    assert_eq!(render("{{ 'a,b,c' | split: ',' | join: '-' }}"), "a-b-c");
    assert_eq!(render("{{ nil | default: 'x' }}"), "x");
    assert_eq!(render("{{ 'hello world' | truncate: 8 }}"), "hello...");
    assert_eq!(render("{{ 1990 | money }}"), "$19.90");
}

#[test]
fn control_flow_and_whitespace() {
    assert_eq!(render("{% if 1 > 2 %}a{% elsif true %}b{% else %}c{% endif %}"), "b");
    assert_eq!(render("{% unless false %}u{% endunless %}"), "u");
    assert_eq!(render("{% case 2 %}{% when 1 %}one{% when 2, 3 %}two{% else %}x{% endcase %}"), "two");
    assert_eq!(render("a  {%- if true -%}  b  {%- endif -%}  c"), "abc");
    assert_eq!(render("{% for i in (1..5) %}{% if i == 4 %}{% break %}{% endif %}{{ i }}{% endfor %}"), "123");
    assert_eq!(render("{% for i in (1..3) reversed %}{{ forloop.index }}{{ i }}{% unless forloop.last %},{% endunless %}{% endfor %}"), "13,22,31");
    assert_eq!(render("{% for i in empty_thing %}x{% else %}none{% endfor %}"), "none");
    assert_eq!(render("{% for i in (1..10) limit: 2 offset: 3 %}{{ i }}{% endfor %}"), "45");
}

#[test]
fn assign_capture_liquid_tag() {
    assert_eq!(render("{% assign a = 'x' | append: 'y' %}{% capture b %}[{{ a }}]{% endcapture %}{{ b }}"), "[xy]");
    assert_eq!(render("{%- liquid\n  assign n = 2\n  if n == 2\n    echo 'two'\n  endif\n-%}"), "two");
    assert_eq!(render("{% for i in (1..2) %}{% assign last = i %}{% endfor %}{{ last }}"), "2");
}

#[test]
fn render_isolates_scope() {
    let html = render_with(
        &[
            ("templates/index.liquid", "{% assign secret = 's' %}{% render 'card', title: 'T' %}{% render 'card' for (1..2) as n %}"),
            ("snippets/card.liquid", "<{{ title }}{{ secret }}{{ n }}>"),
        ],
        "index",
    )
    .html;
    assert_eq!(html, "<T><1><2>");
}

#[test]
fn shop_objects_and_paginate() {
    let html = render_with(
        &[("templates/collection.liquid", "{% paginate collection.products by 2 %}{% for p in collection.products %}{{ p.handle }};{% endfor %}{{ paginate.pages }}{% endpaginate %}")],
        "collection",
    )
    .html;
    assert_eq!(html.matches(';').count(), 2, "{html}");
    assert!(html.ends_with('2'), "{html}");
}

#[test]
fn json_templates_sections_and_blocks() {
    let res = render_with(
        &[
            ("templates/index.json", r#"/* comment */ {"sections": {"hero": {"type": "hero", "settings": {"title": "Hi"}, "blocks": {"b1": {"type": "text", "settings": {"body": "B"}}}, "block_order": ["b1"]}}, "order": ["hero"]}"#),
            ("sections/hero.liquid", "{{ section.settings.title }}|{{ section.settings.size }}|{% content_for 'blocks' %}{% for b in section.blocks %}{{ b.type }}{% endfor %}\n{% schema %}{\"settings\":[{\"id\":\"size\",\"type\":\"range\",\"default\":5}]}{% endschema %}"),
            ("blocks/text.liquid", "<{{ block.settings.body }}>"),
        ],
        "index",
    );
    assert!(res.html.contains("Hi|5|<B>text"), "{}", res.html);
    // Frames: the section, then the outputs inside it.
    let names: Vec<&str> = res.profile.frames.iter().map(|f| f.name.as_str()).collect();
    for want in ["json_template", "raw_section", "liquid_template", "section", "Variable", "Custom", "content_for"] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    let files: Vec<&str> = res.profile.frames.iter().map(|f| &*f.file).collect();
    assert!(files.contains(&"blocks/text") && files.contains(&"sections/hero"), "{files:?}");
}

#[test]
fn missing_snippet_prints_liquid_error() {
    let res = render_with(&[("templates/index.liquid", "a\n{% render 'nope' %}")], "index");
    assert_eq!(res.html, "a\nLiquid error (templates/index line 2): Could not find asset snippets/nope.liquid");
    let names: Vec<&str> = res.profile.frames.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"Render"), "{names:?}");
}

#[test]
fn profile_counts_db_reads_once_per_object() {
    let res = render_with(
        &[("templates/product.liquid", "{{ product.metafields.custom.field_0 }}{{ product.metafields.custom.field_1 }}{% for v in product.variants %}{% endfor %}")],
        "product",
    );
    let ops: HashMap<String, f64> = res.profile.op_totals().into_iter().collect();
    assert_eq!(ops.get("db:product.metafields"), Some(&1.0));
    assert_eq!(ops.get("db:product.variants"), Some(&1.0));
    assert!(ops["iter"] >= 9.0);
}

#[test]
fn calibration_recovers_loop_counts_and_costs() {
    use lq_render::calibrate::{calibrate, Sample};
    let files = vec![
        ("layout/theme.liquid".to_string(), "{{ content_for_layout }}".to_string()),
        (
            "templates/collection.liquid".to_string(),
            "{% for p in collection.products %}\n{% render 'card', p: p %}\n{% endfor %}\n{{ collection.title | upcase }}".to_string(),
        ),
        ("snippets/card.liquid".to_string(), "{{ p.title }}{{ p.featured_image | image_url: width: 400 }}{{ p.price | money }}".to_string()),
    ];
    let theme = Theme::from_files(files);
    let shop = shop();
    let req = PageRequest { template: "collection".into(), handle: None };

    // "Real": the loop ran 7 times on the store, and the store's costs differ from lq's defaults.
    let mut truth = CostModel::builtin();
    for (k, v) in truth.coefficients.iter_mut() {
        *v *= if k.starts_with("filter:") { 2.0 } else { 1.5 };
    }
    let mut real_shapes = HashMap::new();
    real_shapes.insert("templates/collection:1".to_string(), 7);
    let real = render_page(&theme, &shop, &req, &truth, Limits::default(), &real_shapes).unwrap();
    let real_tree = lq_render::speedscope::read(&real.profile.to_speedscope(&truth)).unwrap();

    let prior = CostModel::builtin();
    let samples = [Sample { page: req.clone(), real: real_tree, label: "collection".into() }];
    let c = calibrate(&theme, &shop, &samples, &prior, Limits::default(), HashMap::new()).unwrap();
    assert_eq!(c.shapes.get("templates/collection:1"), Some(&7), "{:?}", c.shape_changes);
    let fit = &c.pages[0];
    assert!((fit.after_ms - fit.real_ms).abs() / fit.real_ms < 0.05, "real {} after {}", fit.real_ms, fit.after_ms);
    assert!((fit.before_ms - fit.real_ms).abs() > (fit.after_ms - fit.real_ms).abs());
    assert!(fit.matched_share > 0.99);
}
