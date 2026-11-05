//! Upstream specs of the args batch that don't go through `check()`: `liquid-doc/*.spec.ts`,
//! `block-parameters.spec.ts`, and the `meta` assertions of check specs.

use lq_check::block_parameters::resolve_block_parameters;
use lq_check::fixes::{fix_all, Corrector, StringCorrector};
use lq_check::liquid_doc::arguments::{add_argument, remove_argument};
use lq_check::liquid_doc::*;
use lq_check::registry::REGISTRY;
use lq_check::Severity;
use lq_json::{parse_json, Value};
use lq_syntax::{parse_strict, Markup, Node};
use std::collections::HashSet;

fn valid_types() -> HashSet<&'static str> {
    BASIC_PARAM_TYPES.iter().copied().chain(["product"]).collect()
}

fn literal(value: &str, raw: &str) -> StringLiteralType {
    StringLiteralType { value: value.into(), raw: raw.into() }
}

// doc-param-type.spec.ts

#[test]
fn parse_doc_param_type_represents_named_and_array_types() {
    let valid = valid_types();
    for name in ["string", "number", "boolean", "object", "product"] {
        assert_eq!(parse_doc_param_type(&valid, name), Some(DocParamType::Named(name.into())));
    }
    for name in ["string", "product"] {
        assert_eq!(parse_doc_param_type(&valid, &format!("{name}[]")), Some(DocParamType::Array(name.into())));
    }
}

#[test]
fn parse_doc_param_type_represents_enums_with_original_spelling() {
    assert_eq!(
        parse_doc_param_type(&valid_types(), "'Heading' | \"small\" | '' | 'a|b' | 'a}b'"),
        Some(DocParamType::Union(vec![
            literal("Heading", "'Heading'"),
            literal("small", "\"small\""),
            literal("", "''"),
            literal("a|b", "'a|b'"),
            literal("a}b", "'a}b'"),
        ]))
    );
}

#[test]
fn parse_doc_param_type_literals_ignore_the_catalog() {
    let empty = HashSet::new();
    assert_eq!(parse_doc_param_type(&empty, "'heading'"), Some(DocParamType::Literal(literal("heading", "'heading'"))));
    assert_eq!(parse_doc_param_type(&empty, "product"), None);
}

#[test]
fn parse_doc_param_type_rejects_unsupported_or_malformed() {
    for value in [
        "", "unknown", "unknown[]", "String", " string ", "string[][]", "string | number", "'heading' |",
        "'heading' | number", "'heading'[]", "'heading' | 'small", "1 | 2",
    ] {
        assert_eq!(parse_doc_param_type(&valid_types(), value), None, "{value:?}");
    }
}

#[test]
fn parse_param_type_tuple_api() {
    let valid = valid_types();
    assert_eq!(parse_param_type(&valid, "product"), Some(("product", false)));
    assert_eq!(parse_param_type(&valid, "product[]"), Some(("product", true)));
    assert_eq!(parse_param_type(&valid, "string"), Some(("string", false)));
    assert_eq!(parse_param_type(&valid, "string[]"), Some(("string", true)));
    assert_eq!(parse_param_type(&valid, "invalid"), None);
    assert_eq!(parse_param_type(&valid, "'heading' | 'small'"), None);
}

#[test]
fn parse_string_literals_keeps_values() {
    let cases: [(&str, &[&str], &[&str]); 8] = [
        ("'heading' | 'small'", &["heading", "small"], &["'heading'", "'small'"]),
        (" \"Heading\"|'small' ", &["Heading", "small"], &["\"Heading\"", "'small'"]),
        ("'heading'", &["heading"], &["'heading'"]),
        ("'' | ' small '", &["", " small "], &["''", "' small '"]),
        ("'a|b'\t|\t'a}b'", &["a|b", "a}b"], &["'a|b'", "'a}b'"]),
        ("\"it's\" | 'say \"hi\"'", &["it's", "say \"hi\""], &["\"it's\"", "'say \"hi\"'"]),
        (r"'a\nb' | 'a\'", &[r"a\nb", r"a\"], &[r"'a\nb'", r"'a\'"]),
        ("'heading' | 'heading'", &["heading", "heading"], &["'heading'", "'heading'"]),
    ];
    for (ty, values, raws) in cases {
        let literals = parse_string_literals(ty).unwrap_or_else(|| panic!("{ty:?}"));
        assert_eq!(literals.iter().map(|l| l.value.as_str()).collect::<Vec<_>>(), values);
        assert_eq!(literals.iter().map(|l| l.raw.as_str()).collect::<Vec<_>>(), raws);
    }
}

#[test]
fn parse_string_literals_rejects_the_entire_annotation() {
    for ty in [
        "", " ", "string", "product[]", "heading | small", "'heading' |", "'heading' | ", "| 'heading'",
        "'heading' || 'small'", "'heading' 'small'", "'heading' | small", "'heading' | 'small", "'heading' trailing",
        "'heading' | number", "1 | 2", "true | false", "('heading' | 'small')", "'heading'[]", "'heading' |\n'small'",
        "'heading\rsmall'", r"'it\'s'",
    ] {
        assert_eq!(parse_string_literals(ty), None, "{ty:?}");
    }
}

// utils.spec.ts

#[test]
fn default_values() {
    let cases = [
        (Some("'Heading' | 'small'"), "'Heading'"),
        (Some(" \"it's\" | 'small' "), "\"it's\""),
        (Some("'' | 'small'"), "''"),
        (Some("'heading' |"), ""),
        (Some("string"), "''"),
        (Some("NUMBER"), "0"),
        (Some("boolean"), "false"),
        (Some("object"), ""),
        (None, ""),
    ];
    for (ty, expected) in cases {
        assert_eq!(default_value_for_type(ty), expected, "{ty:?}");
    }
}

#[test]
fn check_argument_type_cases() {
    use ArgumentTypeCheck::*;
    let named = |t: &str| NamedType(t.to_string());
    let cases = [
        ("'heading' | 'small'", "'heading'", Compatible),
        ("'heading' | 'small'", "\"small\"", Compatible),
        ("'heading' | 'small'", "'Heading'", Incompatible),
        ("'heading' | 'small'", "'large'", Incompatible),
        ("'heading' | 'small'", "42", Incompatible),
        ("'heading' | 'small'", "nil", Incompatible),
        ("'heading' | 'small'", "true", Incompatible),
        ("'heading' | 'small'", "(1..3)", Incompatible),
        ("'heading' | 'small'", "block.settings.variant", Unchecked),
        ("'heading' |", "'small'", Unchecked),
        ("product", "42", named("product")),
        ("Product[]", "42", named("product[]")),
        ("string[]", "'heading'", named("string[]")),
        ("product", "product", Unchecked),
        ("product[", "42", Unchecked),
        ("String", "'heading'", Compatible),
        ("NUMBER", "42", Compatible),
        ("boolean", "'heading'", Compatible),
        ("string", "42", Incompatible),
    ];
    for (ty, value, expected) in cases {
        let source = format!("{{% render 'text', variant: {value} %}}");
        let doc = parse_strict(&source).unwrap();
        let Node::LiquidTag(tag) = &doc.children()[0] else { panic!() };
        let Markup::Node(markup) = &tag.markup else { panic!() };
        let Node::RenderMarkup(render) = &**markup else { panic!() };
        let Node::NamedArgument(arg) = &render.args[0] else { panic!() };
        assert_eq!(check_argument_type(ty, &arg.value), expected, "{ty} against {value}");
    }
}

// arguments.spec.ts

fn with_markup(source: &str, f: impl FnOnce(&mut StringCorrector, &Node<'_>)) -> String {
    let doc = parse_strict(source).unwrap();
    let Node::LiquidTag(tag) = &doc.children()[0] else { panic!() };
    let Markup::Node(markup) = &tag.markup else { panic!() };
    let mut corrector = StringCorrector::default();
    f(&mut corrector, markup);
    fix_all(source, &[&corrector.into_fix()]).unwrap()
}

fn add(source: &str) -> String {
    let param = LiquidDocParameter {
        name: "required_string".into(),
        description: None,
        ty: Some("string".into()),
        required: true,
    };
    with_markup(source, |c, node| add_argument(c, source, node, &param, false))
}

/// `index` from the end of `args`: `at(-index)`.
fn remove(source: &str, from_end: usize) -> String {
    with_markup(source, |c, node| {
        let Node::ContentForMarkup(m) = node else { panic!() };
        remove_argument(c, source, node, &m.args[m.args.len() - from_end]);
    })
}

#[test]
fn add_argument_corrector() {
    let want = "{% content_for 'block', type: 'swatches', id: 'fake', some-arg: '', required_string: '' %}";
    assert_eq!(add("{% content_for 'block', type: 'swatches', id: 'fake', some-arg: '' %}"), want);
    assert_eq!(add("{% content_for 'block', type: 'swatches', id: 'fake', some-arg: '',    %}"), want);
    assert_eq!(add("{% content_for 'block', type: 'swatches', id: 'fake', some-arg: ''    ,    %}"), want);
}

#[test]
fn remove_argument_corrector() {
    let source = "{% content_for 'block', type: 'swatches', id: 'fake', some-arg: '' %}";
    assert_eq!(remove(source, 1), "{% content_for 'block', type: 'swatches', id: 'fake' %}");
    assert_eq!(remove(source, 2), "{% content_for 'block', type: 'swatches', some-arg: '' %}");
    assert_eq!(
        remove("{% content_for 'block', type: 'swatches', id: 'fake', some-arg: '', %}", 1),
        "{% content_for 'block', type: 'swatches', id: 'fake' %}"
    );
    assert_eq!(
        remove("{% content_for 'block', type: 'swatches', id: 'fake'     , some-arg: '' %}", 2),
        "{% content_for 'block', type: 'swatches', some-arg: '' %}"
    );
    assert_eq!(
        remove("{% content_for 'block', type: 'swatches', id: 'fake',    some-arg: '' %}", 1),
        "{% content_for 'block', type: 'swatches', id: 'fake' %}"
    );
}

// liquidDoc.spec.ts

const URI: &str = "file:///snippets/fake.liquid";

fn doc_definition(source: &str) -> DocDefinition {
    let doc = parse_strict(source).unwrap();
    extract_doc_definition(URI, &doc.root)
}

fn doc_content(source: &str) -> DocContent {
    doc_definition(source).liquid_doc.expect("liquidDoc")
}

fn param(name: &str, description: Option<&str>, ty: Option<&str>, required: bool) -> LiquidDocParameter {
    LiquidDocParameter {
        name: name.into(),
        description: description.map(String::from),
        ty: ty.map(String::from),
        required,
    }
}

#[test]
fn doc_keeps_string_enum_spelling_and_optionality() {
    let source = "{% doc %}\n  Renders the block's content as text.\n  @param {'Heading' | \"small\"} [variant] - Shared text style, added as a text--<variant> class; plain text when omitted\n{% enddoc %}";
    assert_eq!(
        doc_content(source).parameters,
        Some(vec![param(
            "variant",
            Some("Shared text style, added as a text--<variant> class; plain text when omitted"),
            Some("'Heading' | \"small\""),
            false,
        )])
    );
}

#[test]
fn doc_without_renderable_content_has_a_description_only() {
    let content = doc_content("\n  {% doc %}\n    just a description\n    @undefined asdf\n  {% enddoc %}\n");
    assert_eq!(
        content,
        DocContent { parameters: None, examples: None, description: Some("just a description".into()) }
    );
}

#[test]
fn doc_param_annotations() {
    let content = doc_content(
        "{% doc %}
          @param {String} firstParam - The first param
          @param {Number} secondParam - The second param
          @param {String} [optionalParam] - The optional param
          @param paramWithNoType - param with no type
          @param paramWithOnlyName
          @param {Number} paramWithNoDescription
        {% enddoc %}",
    );
    assert_eq!(
        content.parameters,
        Some(vec![
            param("firstParam", Some("The first param"), Some("String"), true),
            param("secondParam", Some("The second param"), Some("Number"), true),
            param("optionalParam", Some("The optional param"), Some("String"), false),
            param("paramWithNoType", Some("param with no type"), None, true),
            param("paramWithOnlyName", None, None, true),
            param("paramWithNoDescription", None, Some("Number"), true),
        ])
    );
}

#[test]
fn doc_examples() {
    let examples = |s: &str| doc_content(s).examples;
    assert_eq!(
        examples("{% doc %}\n  @example\n  {{ product }}\n{% enddoc %}"),
        Some(vec!["{{ product }}".to_string()])
    );
    assert_eq!(
        examples("{% doc %}\n          @example\n          {{ product }}\n          {{ product.title }}\n        {% enddoc %}"),
        Some(vec!["{{ product }}\n{{ product.title }}".to_string()])
    );
    assert_eq!(
        examples("{% doc %}\n  @example\n  {{ product }}\n  @example\n  {{ product.title }}\n{% enddoc %}"),
        Some(vec!["{{ product }}".to_string(), "{{ product.title }}".to_string()])
    );
    let both = doc_content(
        "{% doc %}\n  @param {String} product - The product\n  @example\n  {{ product }} // This is an example\n{% enddoc %}",
    );
    assert_eq!(both.parameters, Some(vec![param("product", Some("The product"), Some("String"), true)]));
    assert_eq!(both.examples, Some(vec!["{{ product }} // This is an example".to_string()]));
}

#[test]
fn doc_descriptions() {
    let description = |s: &str| doc_content(s).description;
    assert_eq!(
        description("{% doc %}\n  @description This is a description\n{% enddoc %}").as_deref(),
        Some("This is a description")
    );
    assert_eq!(
        description("{% doc %}\n  @description This is a description\n  @description This is another description\n{% enddoc %}")
            .as_deref(),
        Some("This is a description")
    );
    let implicit = doc_content(
        "{% doc %}
        this is an implicit description
        in a header

        @param asdf

        @description with a description annotation
      {% enddoc %}",
    );
    assert_eq!(implicit.description.as_deref(), Some("this is an implicit description\nin a header"));
    assert_eq!(implicit.parameters, Some(vec![param("asdf", None, None, true)]));
}

#[test]
fn doc_presence() {
    assert_eq!(doc_definition("<div>No doc header here</div>").liquid_doc, None);
    assert_eq!(doc_definition("{% doc %}{% enddoc %}").liquid_doc, Some(DocContent::default()));
}

// block-parameters.spec.ts

fn schema(settings: &str) -> Value {
    parse_json(&format!("{{\"settings\":{settings}}}")).unwrap()
}

fn docs(parameters: Vec<LiquidDocParameter>) -> DocDefinition {
    DocDefinition {
        uri: "file:/blocks/card.liquid".into(),
        liquid_doc: Some(DocContent { parameters: Some(parameters), examples: None, description: None }),
    }
}

#[test]
fn block_parameters_merge_schema_types_with_doc_requiredness() {
    let schema = schema(r#"[{"id":"title","type":"text"},{"id":"product","type":"product"}]"#);
    let doc = docs(vec![
        param("title", None, Some("String"), false),
        param("product", None, Some("object"), true),
        param("tracking_id", None, Some("String"), true),
    ]);
    let parameters = resolve_block_parameters(Some(&schema), Some(&doc));
    let summary: Vec<_> = parameters
        .0
        .iter()
        .map(|p| (p.name.as_str(), p.ty.as_deref(), p.required, p.schema_setting.is_some(), p.liquid_doc.is_some()))
        .collect();
    assert_eq!(
        summary,
        [
            ("content", Some("string"), false, false, false),
            ("title", Some("string"), false, true, true),
            ("product", Some("product"), true, true, true),
            ("tracking_id", Some("string"), true, false, true),
        ]
    );
}

#[test]
fn block_parameters_schema_settings_are_optional() {
    for setting in [
        r#"{"id":"value","type":"checkbox"}"#,
        r#"{"id":"value","type":"range","min":0,"max":10,"step":1,"default":5}"#,
        r#"{"id":"value","type":"image_picker"}"#,
    ] {
        let schema = schema(&format!("[{setting}]"));
        let parameters = resolve_block_parameters(Some(&schema), None);
        assert!(!parameters.get("value").unwrap().required, "{setting}");
    }
}

#[test]
fn block_parameters_content_stays_string_with_doc_requiredness() {
    let doc = docs(vec![param("content", None, Some("Number"), true)]);

    let backed = schema(r#"[{"id":"content","type":"number"}]"#);
    let parameters = resolve_block_parameters(Some(&backed), Some(&doc));
    let content = parameters.get("content").unwrap();
    assert_eq!((content.ty.as_deref(), content.required), (Some("string"), true));
    assert!(content.schema_setting.is_some() && content.liquid_doc.is_some());

    let empty = schema("[]");
    let parameters = resolve_block_parameters(Some(&empty), Some(&doc));
    let content = parameters.get("content").unwrap();
    assert_eq!((content.ty.as_deref(), content.required), (Some("string"), true));
    assert!(content.schema_setting.is_none());
}

#[test]
fn block_parameters_normalize_doc_only_types() {
    let doc = docs(vec![
        param("products", None, Some("Product[]"), true),
        param("untyped", None, None, false),
        param("malformed", None, Some("not a type"), false),
    ]);
    let parameters = resolve_block_parameters(None, Some(&doc));
    assert_eq!(parameters.get("products").unwrap().ty.as_deref(), Some("product[]"));
    assert_eq!(parameters.get("untyped").unwrap().ty, None);
    assert_eq!(parameters.get("malformed").unwrap().ty, None);
}

#[test]
fn block_parameters_unmapped_setting_type_stays_untyped() {
    let schema = schema(r#"[{"id":"item","type":"metaobject"}]"#);
    let doc = docs(vec![param("item", None, Some("metaobject"), true)]);
    let parameters = resolve_block_parameters(Some(&schema), Some(&doc));
    let item = parameters.get("item").unwrap();
    assert!(item.required);
    assert_eq!(item.ty, None);
}

// `meta` assertions of check specs.

#[test]
fn recommended_errors() {
    for code in ["BlockContentUsage", "ValidBlockContentSettingType", "ValidBlockTagPlacement"] {
        let def = REGISTRY.iter().find(|d| d.meta.code == code).unwrap();
        assert!(def.meta.recommended, "{code}");
        assert_eq!(def.meta.severity, Severity::Error, "{code}");
    }
}
