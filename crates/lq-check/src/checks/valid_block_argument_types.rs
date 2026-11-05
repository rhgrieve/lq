//! `ValidBlockArgumentTypes`.

use super::shared_args::{block_name, is_content_parameter, named, valid_block_markup};
use crate::api::*;
use crate::block_parameters::{BlockParameter, BlockParameters};
use crate::liquid_doc::arguments::with_type_mismatch_suggestions;
use crate::liquid_doc::{
    argument_type_mismatch_message, check_argument_type, infer_argument_type, is_type_compatible, liquid_doc_type,
    parse_string_literals, ArgumentTypeCheck,
};
use crate::to_schema::is_block;
use lq_syntax::{LiquidDocParamNode, NamedArgument, Node};
use std::sync::Arc;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag, NodeKind::LiquidDocParamNode]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::<Check<'a>>::default())
}

/// What `await getBlockParameters(currentBlockName)` costs against the plain calls of
/// other checks. Measured on `themeCheckRun`: `ValidBlockContentSettingType`'s report
/// on the same block comes first with up to 6 raw tags before `{% schema %}`, after
/// with 7 (a tie then goes to this check). Everything later in this check's pipeline is delayed by it too.
const CURRENT_PARAMETERS_ROUNDS: u32 = 8;

#[derive(Default)]
struct Check<'a> {
    current_parameters: Option<Option<Arc<BlockParameters<'a>>>>,
}

impl<'a> LiquidCheck<'a> for Check<'a> {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        if self.current_parameters.is_some() {
            cx.delay_reports(CURRENT_PARAMETERS_ROUNDS);
        }
        match node {
            Node::LiquidTag(tag) => {
                let Some(markup) = valid_block_markup(tag, cx.source()) else { return };
                let Some(parameters) = cx.block_parameters(block_name(markup)) else { return };
                for arg in &markup.args {
                    if let Some(a) = named(arg) {
                        check_argument(a, &parameters, cx);
                    }
                }
            }
            Node::LiquidDocParamNode(param) => {
                if !is_block(cx.uri()) {
                    return;
                }
                let parameters = match &mut self.current_parameters {
                    Some(parameters) => parameters,
                    slot => {
                        cx.delay_reports(CURRENT_PARAMETERS_ROUNDS);
                        slot.insert(cx.block_parameters(&lq_json::uri::basename(cx.uri(), Some(".liquid"))))
                    }
                };
                let Some(parameters) = parameters else { return };
                let Node::TextNode(name) = &param.param_name else { return };
                if let Some(parameter) = parameters.get(&name.value) {
                    report_liquid_doc_type_mismatch(param, parameter, cx);
                }
            }
            _ => {}
        }
    }
}

fn check_argument<'a>(a: &NamedArgument<'a>, parameters: &BlockParameters<'_>, cx: &LiquidCx<'a, '_>) {
    let expected = parameters.get(&a.name).and_then(|p| p.ty.as_deref()).filter(|t| !t.is_empty());
    if let Some(expected) = expected.filter(|t| parse_string_literals(t).is_some()) {
        if check_argument_type(expected, &a.value) == ArgumentTypeCheck::Incompatible {
            let p = a.value.position();
            let message = argument_type_mismatch_message(&a.name, expected, &a.value, cx.source());
            cx.report(with_type_mismatch_suggestions(LiquidProblem::new(message, p.start, p.end), expected, p.start, p.end));
        }
        return;
    }
    let Some(expected) = expected else { return };
    let Some(actual) = literal_type(&a.value) else { return };
    if is_call_type_compatible(expected, &actual) {
        return;
    }
    let p = a.value.position();
    cx.report(LiquidProblem::new(
        format!("Type mismatch for argument '{}': expected {expected}, got {actual}", a.name),
        p.start,
        p.end,
    ));
}

/// `literalType(value)`. Lookups and empty or partly unknown arrays are `None`.
fn literal_type(value: &Node<'_>) -> Option<String> {
    let elements = match value {
        Node::VariableLookup(_) => return None,
        Node::BlockArrayLiteral(array) => &array.elements,
        _ => return Some(infer_argument_type(value).to_string()),
    };
    let mut types: Vec<String> = Vec::new();
    for element in elements {
        let ty = literal_type(element)?;
        if !types.contains(&ty) {
            types.push(ty);
        }
    }
    match types.as_slice() {
        [] => None,
        [only] => Some(format!("{only}[]")),
        _ => Some("mixed[]".to_string()),
    }
}

/// `isCallTypeCompatible(expectedType, actualType)`.
fn is_call_type_compatible(expected: &str, actual: &str) -> bool {
    if expected == "object" {
        return true;
    }
    if actual == "object" || actual.ends_with("[]") {
        return expected == actual;
    }
    is_type_compatible(expected, actual)
}

fn report_liquid_doc_type_mismatch<'a>(param: &LiquidDocParamNode<'a>, parameter: &BlockParameter<'_>, cx: &LiquidCx<'a, '_>) {
    let Some(param_type) = &param.param_type else { return };
    let Node::TextNode(type_text) = param_type else { return };
    let Some(declared) = liquid_doc_type(Some(&type_text.value)) else { return };
    let is_content = is_content_parameter(&parameter.name);
    if parameter.schema_setting.is_none() && !is_content {
        return;
    }
    let Some(authoritative) = parameter.ty.as_deref().filter(|t| !t.is_empty()) else { return };
    if declared == "object" || declared == authoritative {
        return;
    }
    let source = if is_content { "built-in parameter" } else { "schema setting" };
    let p = param_type.position();
    cx.report(LiquidProblem::new(
        format!(
            "The {source} '{}' has Liquid type '{authoritative}', but LiquidDoc declares '{declared}'. The {source} type is authoritative.",
            parameter.name
        ),
        p.start - 1,
        p.end + 1,
    ));
}
