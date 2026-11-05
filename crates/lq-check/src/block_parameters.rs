//! `block-parameters.ts`.

use crate::deps::Dependencies;
use crate::liquid_doc::{liquid_doc_type, parse_string_literals, DocDefinition, LiquidDocParameter};
use crate::schema_settings::schema_setting_liquid_type;
use lq_json::Value;

pub const BLOCK_CONTENT_PARAMETER: &str = "content";

/// `BlockParameter`.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockParameter<'a> {
    pub name: String,
    pub ty: Option<String>,
    pub required: bool,
    pub schema_setting: Option<&'a Value>,
    pub liquid_doc: Option<&'a LiquidDocParameter>,
}

/// `Map<string, BlockParameter>` in insertion order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockParameters<'a>(pub Vec<BlockParameter<'a>>);

impl<'a> BlockParameters<'a> {
    pub fn get(&self, name: &str) -> Option<&BlockParameter<'a>> {
        self.0.iter().find(|p| p.name == name)
    }

    fn set(&mut self, param: BlockParameter<'a>) {
        match self.0.iter_mut().find(|p| p.name == param.name) {
            Some(slot) => *slot = param,
            None => self.0.push(param),
        }
    }
}

fn is_input_setting(setting: &Value) -> bool {
    setting.get("id").and_then(Value::as_str).is_some()
        && !matches!(setting.get("type").and_then(Value::as_str), Some("header" | "paragraph"))
}

/// `resolveBlockParameters(schema, docDefinition)`.
pub fn resolve_block_parameters<'a>(
    schema: Option<&'a Value>,
    doc: Option<&'a DocDefinition>,
) -> BlockParameters<'a> {
    let mut params = BlockParameters(vec![BlockParameter {
        name: BLOCK_CONTENT_PARAMETER.into(),
        ty: Some("string".into()),
        required: false,
        schema_setting: None,
        liquid_doc: None,
    }]);
    if let Some(settings) = schema.and_then(|s| s.get("settings")).and_then(Value::as_array) {
        for setting in settings {
            if !is_input_setting(setting) {
                continue;
            }
            let id = setting.get("id").and_then(Value::as_str).unwrap_or("");
            let setting_type = setting.get("type").and_then(Value::as_str).unwrap_or("");
            params.set(BlockParameter {
                name: id.to_string(),
                ty: if id == BLOCK_CONTENT_PARAMETER {
                    Some("string".into())
                } else {
                    schema_setting_liquid_type(setting_type).map(String::from)
                },
                required: false,
                schema_setting: Some(setting),
                liquid_doc: None,
            });
        }
    }
    let docs = doc.and_then(|d| d.liquid_doc.as_ref()).and_then(|d| d.parameters.as_ref());
    for liquid_doc in docs.into_iter().flatten() {
        let param = match params.get(&liquid_doc.name) {
            Some(existing) => BlockParameter {
                required: liquid_doc.required,
                liquid_doc: Some(liquid_doc),
                ..existing.clone()
            },
            None => BlockParameter {
                name: liquid_doc.name.clone(),
                ty: match &liquid_doc.ty {
                    Some(t) if !t.is_empty() && parse_string_literals(t).is_some() => {
                        Some(lq_syntax::js::trim(t).to_string())
                    }
                    t => liquid_doc_type(t.as_deref()),
                },
                required: liquid_doc.required,
                schema_setting: None,
                liquid_doc: Some(liquid_doc),
            },
        };
        params.set(param);
    }
    params
}

/// The `load` of `makeGetBlockParameters`.
pub(crate) fn make_block_parameters<'a>(
    deps: &'a dyn Dependencies<'a>,
    block_name: &str,
) -> Option<BlockParameters<'a>> {
    let schema = deps.block_schema(block_name)?;
    let doc = deps.doc_definition(&format!("blocks/{block_name}.liquid"))?;
    match schema.valid_schema() {
        Some(Ok(v)) => Some(resolve_block_parameters(v.as_ref(), Some(doc))),
        // App block schemas have no `validSchema`: `undefined` is not an Error.
        None => Some(resolve_block_parameters(None, Some(doc))),
        Some(Err(_)) => schema.has_no_schema_tag().then(|| resolve_block_parameters(None, Some(doc))),
    }
}

/// `getBlockParameters(name)` outside a run, for debug tooling.
pub fn make_for_debug<'a>(deps: &'a dyn Dependencies<'a>, block_name: &str) -> Option<BlockParameters<'a>> {
    make_block_parameters(deps, block_name)
}
