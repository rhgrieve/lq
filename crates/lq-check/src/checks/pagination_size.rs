//! `PaginationSize`.

use super::shared_rules::{js_string_to_number, js_to_number, js_to_string, parsed_schema_body, type_error};
use crate::api::*;
use lq_json::js::number_to_string;
use lq_json::Value;
use lq_syntax::{LiquidVariableLookup, Markup, Node, Position};

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag, NodeKind::LiquidRawTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(PaginationSize {
        min: cx.settings().f64("minSize").unwrap_or(f64::NAN),
        max: cx.settings().f64("maxSize").unwrap_or(f64::NAN),
        schema: None,
        lookups: Vec::new(),
    }))
}

struct PaginationSize<'a> {
    min: f64,
    max: f64,
    /// The last schema whose `settings` is an array.
    schema: Option<Value>,
    lookups: Vec<(&'a LiquidVariableLookup<'a>, Position)>,
}

impl PaginationSize<'_> {
    fn within(&self, value: f64) -> bool {
        self.min <= value && value <= self.max
    }
}

impl<'a> LiquidCheck<'a> for PaginationSize<'a> {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        match node {
            Node::LiquidTag(tag) if tag.name == "paginate" => {
                let Markup::Node(markup) = &tag.markup else { return };
                let Node::PaginateMarkup(paginate) = &**markup else { return };
                let position = paginate.page_size.position();
                match &paginate.page_size {
                    Node::VariableLookup(lookup) => self.lookups.push((lookup, position)),
                    Node::Number(number) if !self.within(js_string_to_number(number.value)) => {
                        cx.report(LiquidProblem::new(
                            format!(
                                "Pagination size must be a positive integer between {} and {}.",
                                number_to_string(self.min),
                                number_to_string(self.max)
                            ),
                            position.start,
                            position.end,
                        ));
                    }
                    _ => {}
                }
            }
            Node::LiquidRawTag(tag) if tag.name == "schema" => {
                let Some(schema) = parsed_schema_body(cx, tag, "settings") else { return };
                if schema.get("settings").is_some_and(|s| s.as_array().is_some()) {
                    self.schema = Some(schema);
                }
            }
            _ => {}
        }
    }

    fn on_code_path_end(&mut self, cx: &LiquidCx<'a, '_>) {
        let settings = self.schema.as_ref().and_then(|s| s.get("settings")).and_then(Value::as_array).unwrap_or(&[]);
        for &(lookup, position) in &self.lookups {
            let Some(Node::String(last)) = lookup.lookups.last() else { continue };
            let mut setting = None;
            for candidate in settings {
                // `setting.id` throws on a null item.
                if candidate.is_null() {
                    if cx.divergence(Divergence::NullInPath) {
                        continue;
                    }
                    cx.fail(type_error("Cannot read properties of null (reading 'id')"));
                    return;
                }
                if candidate.get("id").and_then(Value::as_str) == Some(last.value) {
                    setting = Some(candidate);
                    break;
                }
            }
            let Some(setting) = setting else { continue };
            let Some(default) = setting.get("default") else {
                cx.report(LiquidProblem::new(
                    "Default pagination size should be defined in the section settings.",
                    position.start,
                    position.end,
                ));
                continue;
            };
            if !self.within(js_to_number(default)) {
                cx.report(LiquidProblem::new(
                    format!(
                        "This setting's default value should be between {} and {} but is currently {}.",
                        number_to_string(self.min),
                        number_to_string(self.max),
                        js_to_string(default)
                    ),
                    position.start,
                    position.end,
                ));
            }
        }
    }
}
