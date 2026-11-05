//! Upstream unit specs that don't go through `check()`, ported.

use lq_check::liquid_doc::*;
use lq_check::run::check_with_registry;
use lq_check::schema_settings::schema_setting_liquid_type;
use lq_check::source::to_source_code;
use lq_check::utils::styles::collect_used_classes;
use lq_check::visitor::find_current_node;
use lq_syntax::{parse_strict, Node};

#[test]
fn doc_param_type() {
    assert!(parse_string_literals("'a' | \"b\"").is_some_and(|l| l.len() == 2 && l[1].raw == "\"b\""));
    assert!(parse_string_literals("'a' |").is_none());
    assert!(parse_string_literals("'a' b").is_none());
    assert!(parse_string_literals("string").is_none());
    assert!(parse_string_literals("").is_none());
    assert_eq!(parse_param_type_syntax("product[]"), Some(("product", true)));
    assert_eq!(parse_param_type_syntax("Product"), None);
    assert_eq!(normalize_named_param_type("Product[]").as_deref(), Some("product[]"));
    assert_eq!(default_value_for_type(Some("'x' | 'y'")), "'x'");
    assert_eq!(default_value_for_type(Some("String")), "''");
    assert_eq!(default_value_for_type(None), "");
}

#[test]
fn argument_type_checks() {
    let doc = parse_strict("{% render 'a', x: 1, y: 'z', v: w %}").unwrap();
    let Node::LiquidTag(tag) = &doc.children()[0] else { panic!() };
    let lq_syntax::Markup::Node(m) = &tag.markup else { panic!() };
    let Node::RenderMarkup(r) = &**m else { panic!() };
    let value = |i: usize| match &r.args[i] {
        Node::NamedArgument(a) => &a.value,
        _ => panic!(),
    };
    assert_eq!(check_argument_type("string", value(0)), ArgumentTypeCheck::Incompatible);
    assert_eq!(check_argument_type("boolean", value(0)), ArgumentTypeCheck::Compatible);
    assert_eq!(check_argument_type("'z'", value(1)), ArgumentTypeCheck::Compatible);
    assert_eq!(check_argument_type("product", value(1)), ArgumentTypeCheck::NamedType("product".into()));
    assert_eq!(check_argument_type("number", value(2)), ArgumentTypeCheck::Unchecked);
}

#[test]
fn doc_definition() {
    let source = "{% doc %}\n  Intro\n  @param {string} a - first\n  @param [b] - second\n  @example\n    x\n      y\n{% enddoc %}";
    let doc = parse_strict(source).unwrap();
    let def = extract_doc_definition("file:///snippets/a.liquid", &doc.root);
    let content = def.liquid_doc.unwrap();
    let params = content.parameters.unwrap();
    assert_eq!(params[0].name, "a");
    assert_eq!(params[0].ty.as_deref(), Some("string"));
    assert!(params[0].required);
    assert!(!params[1].required);
    assert_eq!(content.examples.unwrap()[0], "x\ny");
    assert_eq!(content.description.as_deref(), Some("Intro"));
    let plain = parse_strict("hello").unwrap();
    assert!(extract_doc_definition("u", &plain.root).liquid_doc.is_none());
}

#[test]
fn schema_settings() {
    assert_eq!(schema_setting_liquid_type("checkbox"), Some("boolean"));
    assert_eq!(schema_setting_liquid_type("product_list"), Some("product[]"));
    assert_eq!(schema_setting_liquid_type("header"), None);
}

#[test]
fn used_classes() {
    let source = "<div class=\"btn-{{ size }} active\tx {{ a }}-b\"></div>";
    let doc = parse_strict(source).unwrap();
    let Node::HtmlElement(el) = &doc.children()[0] else { panic!() };
    let mut used = Vec::new();
    collect_used_classes(&el.attributes, &mut used);
    let names: Vec<&str> = used.iter().map(|c| c.class_name.as_str()).collect();
    assert_eq!(names, ["active", "x"]);
    assert_eq!(&source[used[0].start as usize..used[0].end as usize], "active");
}

#[test]
fn current_node() {
    let source = "{% if a %}{{ b }}{% endif %}";
    let doc = parse_strict(source).unwrap();
    let (node, ancestors) = find_current_node(&doc.root, 14);
    assert_eq!(node.node_type(), "VariableLookup");
    assert!(!ancestors.is_empty());
}

#[test]
fn json_source_parses_or_fails() {
    assert!(matches!(to_source_code("file:///a.json", "{").ast, lq_check::Ast::Json(Err(_))));
    assert!(matches!(to_source_code("file:///a.liquid", "{% if %}").ast, lq_check::Ast::Liquid(Err(_))));
    let _ = check_with_registry; // the entry point is public for custom registries
}
