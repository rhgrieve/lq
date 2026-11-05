//! `check()`: the runner. All Liquid checks on a file share one AST walk, dispatched
//! by node kind so a node pays only for checks interested in it. Files run in
//! parallel when called within a rayon pool.

use crate::api::*;
use crate::block_parameters::{make_block_parameters, BlockParameters};
use crate::context_utils::{self, MetafieldDefinitionMap};
use crate::deps::Dependencies;
use crate::disabled_checks::DisabledChecks;
use crate::fixes::Fix;
use crate::source::{Ast, SourceCode, Theme};
use crate::visitor::{for_each_json_child, for_each_visit_child};
use lq_json::validator::JsError;
use lq_json::{uri, JsonNode, Object, Value};
use lq_syntax::{Node, SourceMap};
use lq_theme::config::Config;
use lq_theme::docs::{AugmentedThemeDocset, ThemeDocset};
use lq_theme::ignore::is_ignored;
use rayon::prelude::*;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, OnceLock};

/// `Position`. `index` is in UTF-16 units; `line` and `character` are 0-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub index: u32,
    pub line: u32,
    pub character: u32,
}

/// `Offense`.
#[derive(Clone, Debug, PartialEq)]
pub struct Offense {
    pub kind: SourceCodeType,
    pub check: &'static str,
    pub message: String,
    pub uri: String,
    pub severity: Severity,
    pub start: Position,
    pub end: Position,
    /// Byte offsets of the problem in the file.
    pub start_byte: u32,
    pub end_byte: u32,
    pub fix: Option<Fix>,
    pub suggest: Option<Vec<Suggestion>>,
    pub(crate) tick: u32,
    pub(crate) check_index: u16,
    pub(crate) file_index: u32,
}

/// The result of a run.
#[derive(Debug, Default)]
pub struct RunOutput {
    pub offenses: Vec<Offense>,
    /// Rejected check pipelines: `config.onError` upstream, silent by default.
    pub errors: Vec<(&'static str, String, JsError)>,
}

/// Shared by all checks and files of a run. Lazily computes and caches what
/// upstream's dependencies produce.
pub struct RunContext<'a> {
    pub theme: &'a Theme<'a>,
    pub config: &'a Config,
    pub deps: &'a dyn Dependencies<'a>,
    docset: Option<&'a dyn ThemeDocset>,
    augmented: Option<AugmentedThemeDocset<'a>>,
    default_locale: OnceLock<String>,
    default_schema_locale: OnceLock<String>,
    default_translations: OnceLock<Option<Value>>,
    default_schema_translations: OnceLock<Option<Value>>,
    metafields: OnceLock<MetafieldDefinitionMap>,
    block_parameters: Mutex<HashMap<String, Option<Arc<BlockParameters<'a>>>>>,
    /// `ValidScopedCSSClass`'s theme-wide sets, which upstream recomputes per file.
    pub(crate) theme_css: OnceLock<crate::checks::shared_schema::ThemeCssClasses>,
    root_prefix: String,
}

impl<'a> RunContext<'a> {
    pub fn new(theme: &'a Theme<'a>, config: &'a Config, deps: &'a dyn Dependencies<'a>) -> Self {
        let docset = deps.theme_docset();
        let augmented = docset.filter(|d| !d.is_augmented()).map(AugmentedThemeDocset::new);
        RunContext {
            theme,
            config,
            deps,
            docset,
            augmented,
            default_locale: OnceLock::new(),
            default_schema_locale: OnceLock::new(),
            default_translations: OnceLock::new(),
            default_schema_translations: OnceLock::new(),
            metafields: OnceLock::new(),
            block_parameters: Mutex::new(HashMap::new()),
            theme_css: OnceLock::new(),
            root_prefix: config.root_uri.trim_end_matches('/').to_string(),
        }
    }

    pub(crate) fn docset(&self) -> Option<&dyn ThemeDocset> {
        match &self.augmented {
            Some(a) => Some(a),
            None => self.docset,
        }
    }

    /// `path.join(config.rootUri, ...relativePath.split('/'))`.
    pub fn to_uri(&self, relative_path: &str) -> String {
        let simple = !relative_path.is_empty()
            && !relative_path.contains(['%', '?', '#', '\\'])
            && !relative_path.starts_with('/')
            && !relative_path.ends_with('/')
            && !relative_path.split('/').any(|s| s.is_empty() || s == "." || s == "..");
        if simple {
            let mut s = String::with_capacity(self.root_prefix.len() + relative_path.len() + 1);
            s.push_str(&self.root_prefix);
            s.push('/');
            s.push_str(relative_path);
            return s;
        }
        let parts: Vec<&str> = relative_path.split('/').collect();
        uri::join(&self.config.root_uri, &parts)
    }

    pub(crate) fn default_locale(&self) -> &str {
        self.default_locale.get_or_init(|| {
            context_utils::default_locale(self.deps.fs(), &self.config.root_uri, ".default.json")
        })
    }

    pub(crate) fn default_schema_locale(&self) -> &str {
        self.default_schema_locale.get_or_init(|| {
            context_utils::default_locale(self.deps.fs(), &self.config.root_uri, ".default.schema.json")
        })
    }

    pub(crate) fn default_translations(&self) -> Option<&Value> {
        self.default_translations
            .get_or_init(|| {
                context_utils::default_translations(self.deps.fs(), self.theme, &self.config.root_uri, ".default.json")
            })
            .as_ref()
    }

    pub(crate) fn default_schema_translations(&self) -> Option<&Value> {
        self.default_schema_translations
            .get_or_init(|| {
                context_utils::default_translations(self.deps.fs(), self.theme, &self.config.root_uri, ".default.schema.json")
            })
            .as_ref()
    }

    pub(crate) fn metafield_definitions(&self) -> &MetafieldDefinitionMap {
        self.metafields.get_or_init(|| {
            self.deps.metafield_definitions(&self.config.root_uri).unwrap_or_else(|| {
                context_utils::metafield_definitions(self.deps.fs(), &self.config.root_uri)
            })
        })
    }

    pub(crate) fn block_parameters(&self, name: &str) -> Option<Arc<BlockParameters<'a>>> {
        if let Some(v) = self.block_parameters.lock().unwrap().get(name) {
            return v.clone();
        }
        let computed = make_block_parameters(self.deps, name).map(Arc::new);
        self.block_parameters.lock().unwrap().entry(name.to_string()).or_insert(computed).clone()
    }
}

/// The per-file mutable state behind `context.report`.
pub struct FileState<'a> {
    file_index: u32,
    source_map: OnceCell<SourceMap<'a>>,
    offenses: RefCell<Vec<Offense>>,
    failed: RefCell<Vec<(u16, JsError)>>,
    any_failed: Cell<bool>,
    tick: Cell<u32>,
}

impl<'a> FileState<'a> {
    fn new(file_index: u32) -> Self {
        FileState {
            file_index,
            source_map: OnceCell::new(),
            offenses: RefCell::new(Vec::new()),
            failed: RefCell::new(Vec::new()),
            any_failed: Cell::new(false),
            tick: Cell::new(0),
        }
    }

    fn position(&self, source: &'a str, byte: u32) -> Position {
        let map = self.source_map.get_or_init(|| SourceMap::new(source));
        let len = source.len() as u32;
        let clamped = byte.min(len);
        let lc = map.line_character(clamped);
        // Two bytes into an astral char stands for its second UTF-16 unit, which only
        // a parse error can point at (upstream clamps to `source.length - 1`).
        let low_surrogate = u32::from(is_second_utf16_unit(source, clamped as usize));
        Position {
            index: map.utf16_index(clamped) + (byte - clamped) + low_surrogate,
            line: lc.line - 1,
            character: lc.character + low_surrogate,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn report(
        &self,
        def: &'static CheckDef,
        index: u16,
        severity: Severity,
        file: &'a SourceCode<'a>,
        message: String,
        start: u32,
        end: u32,
        fix: Option<Fix>,
        suggest: Option<Vec<Suggestion>>,
    ) {
        let start_pos = self.position(file.source, start);
        let end_pos = self.position(file.source, end);
        self.offenses.borrow_mut().push(Offense {
            kind: def.meta.kind,
            check: def.meta.code,
            message,
            uri: file.uri.clone(),
            severity,
            start: start_pos,
            end: end_pos,
            start_byte: start,
            end_byte: end,
            fix,
            suggest,
            tick: self.tick.get(),
            check_index: index,
            file_index: self.file_index,
        });
    }

    /// Pushes this call's reports `rounds` further in offense order.
    pub(crate) fn delay(&self, rounds: u32) {
        self.tick.set(self.tick.get() + rounds);
    }

    pub(crate) fn fail(&self, index: u16, error: JsError) {
        self.failed.borrow_mut().push((index, error));
        self.any_failed.set(true);
    }
}

/// A check's run-wide data.
struct Prepared {
    def: &'static CheckDef,
    settings: Settings,
    severity: Severity,
    code: &'static str,
    /// The index among all prepared checks of this run; offense order uses it.
    index: u16,
}

/// `createSettings`.
fn create_settings(def: &CheckDef, check_settings: Option<&Object>) -> Settings {
    let mut out = Object::new();
    for (key, prop) in def.meta.schema {
        let configured = check_settings.and_then(|s| s.get(key)).filter(|v| !v.is_null());
        if let Some(v) = configured {
            out.insert(*key, v.clone());
        } else if let Some(d) = prop.default {
            out.insert(*key, d.to_value());
        }
    }
    Settings(out)
}

fn prepare(config: &Config, registry: &'static [CheckDef]) -> Vec<Prepared> {
    let mut out = Vec::with_capacity(config.checks.len());
    let mut used = vec![false; registry.len()];
    for code in &config.checks {
        // A code is shared by two definitions at most (MaxFileSize); the config's
        // list has one entry per definition, in registry order.
        let Some(i) = registry.iter().enumerate().position(|(i, d)| !used[i] && d.meta.code == *code) else {
            continue;
        };
        used[i] = true;
        let def = &registry[i];
        let check_settings = config.check_settings(code);
        let severity = check_settings
            .and_then(|s| s.get("severity"))
            .and_then(Value::as_f64)
            .and_then(Severity::from_number)
            .unwrap_or(def.meta.severity);
        out.push(Prepared {
            def,
            settings: create_settings(def, check_settings),
            severity,
            code: def.meta.code,
            index: out.len() as u16,
        });
    }
    out
}

/// Per-node-kind lists of slots, so dispatch allocates nothing and tests nothing per check.
struct JsonTable {
    enter: [Vec<u16>; JsonKind::COUNT],
    exit: [Vec<u16>; JsonKind::COUNT],
    entries: Vec<(usize, JsonCreate)>,
}

const ALL_LIQUID_KINDS: [NodeKind; NodeKind::COUNT] = {
    // Safety: `NodeKind` is repr(u8) with dense discriminants below COUNT.
    let mut out = [NodeKind::Document; NodeKind::COUNT];
    let mut i = 0;
    while i < NodeKind::COUNT {
        out[i] = unsafe { std::mem::transmute::<u8, NodeKind>(i as u8) };
        i += 1;
    }
    out
};

const ALL_JSON_KINDS: [JsonKind; JsonKind::COUNT] =
    [JsonKind::Object, JsonKind::Array, JsonKind::Property, JsonKind::Identifier, JsonKind::Literal];

/// `check(theme, config, dependencies)` over the registry.
pub fn check<'a>(theme: &'a Theme<'a>, config: &'a Config, deps: &'a dyn Dependencies<'a>) -> RunOutput {
    check_with_registry(theme, config, deps, crate::registry::REGISTRY)
}

/// The offense order upstream produces when no check awaits real I/O: pipelines
/// advance one awaited call per microtask round, JSON files before Liquid files.
fn order_key(o: &Offense) -> (u32, u8, u32, u16) {
    (o.tick, u8::from(o.kind == SourceCodeType::LiquidHtml), o.file_index, o.check_index)
}

pub fn check_with_registry<'a>(
    theme: &'a Theme<'a>,
    config: &'a Config,
    deps: &'a dyn Dependencies<'a>,
    registry: &'static [CheckDef],
) -> RunOutput {
    let run = RunContext::new(theme, config, deps);
    let prepared = prepare(config, registry);

    let mut liquid_entries: Vec<(usize, LiquidCreate)> = Vec::new();
    let mut liquid_enter: Vec<Vec<u16>> = vec![Vec::new(); NodeKind::COUNT];
    let mut liquid_exit: Vec<Vec<u16>> = vec![Vec::new(); NodeKind::COUNT];
    let mut json = JsonTable {
        enter: Default::default(),
        exit: Default::default(),
        entries: Vec::new(),
    };
    for (pi, p) in prepared.iter().enumerate() {
        match p.def.imp {
            CheckImpl::NotImplemented => {}
            CheckImpl::Liquid { create, enter, exit } => {
                let slot = liquid_entries.len() as u16;
                liquid_entries.push((pi, create));
                for kind in ALL_LIQUID_KINDS {
                    if enter.contains(kind) {
                        liquid_enter[kind as usize].push(slot);
                    }
                    if exit.contains(kind) {
                        liquid_exit[kind as usize].push(slot);
                    }
                }
            }
            CheckImpl::Json { create, enter, exit } => {
                let slot = json.entries.len() as u16;
                json.entries.push((pi, create));
                for kind in ALL_JSON_KINDS {
                    if enter.contains(kind) {
                        json.enter[kind as usize].push(slot);
                    }
                    if exit.contains(kind) {
                        json.exit[kind as usize].push(slot);
                    }
                }
            }
        }
    }
    let liquid = LiquidDispatch { enter: liquid_enter, exit: liquid_exit, entries: liquid_entries };

    let per_file = |(file_index, file): (usize, &'a SourceCode<'a>)| -> FileResult {
        match &file.ast {
            Ast::Liquid(_) => run_liquid_file(&run, &prepared, &liquid, file, file_index as u32),
            Ast::Json(_) => run_json_file(&run, &prepared, &json, file, file_index as u32),
        }
    };
    let results: Vec<FileResult> = theme.par_iter().enumerate().map(per_file).collect();

    let mut output = RunOutput::default();
    let mut offenses = Vec::new();
    for r in results {
        offenses.extend(r.offenses);
        for (index, err) in r.errors {
            output.errors.push((prepared[index as usize].code, r.uri.clone(), err));
        }
    }
    offenses.sort_by_key(order_key);
    output.offenses = offenses;
    output
}

/// Whether `at` is two bytes into a four-byte char.
#[inline]
fn is_second_utf16_unit(source: &str, at: usize) -> bool {
    !source.is_char_boundary(at) && at >= 2 && source.as_bytes()[at - 2] >= 0xF0
}

struct FileResult {
    uri: String,
    offenses: Vec<Offense>,
    errors: Vec<(u16, JsError)>,
}

struct LiquidDispatch {
    enter: Vec<Vec<u16>>,
    exit: Vec<Vec<u16>>,
    entries: Vec<(usize, LiquidCreate)>,
}

struct LiquidWalk<'a, 'f> {
    cxs: &'f [Option<LiquidCx<'a, 'f>>],
    checks: &'f mut [Option<Box<dyn LiquidCheck<'a> + 'a>>],
    dispatch: &'f LiquidDispatch,
    ancestors: Vec<&'a Node<'a>>,
    disabled: Option<&'f mut DisabledChecks>,
    root: &'a Node<'a>,
    state: &'f FileState<'a>,
    /// Per check: how many calls it has had, upstream's "tick" for offense order.
    calls: &'f mut [u32],
}

impl<'a> LiquidWalk<'a, '_> {
    #[inline]
    fn call(&mut self, slot: usize, f: impl FnOnce(&mut (dyn LiquidCheck<'a> + 'a), &LiquidCx<'a, '_>)) {
        if let (Some(check), Some(cx)) = (self.checks[slot].as_deref_mut(), self.cxs[slot].as_ref()) {
            self.calls[slot] += 1;
            self.state.tick.set(self.calls[slot]);
            f(check, cx);
        }
    }

    fn sweep_failed(&mut self) {
        if self.state.any_failed.get() {
            self.state.any_failed.set(false);
            for (index, _) in self.state.failed.borrow().iter() {
                for (slot, cx) in self.cxs.iter().enumerate() {
                    if cx.as_ref().is_some_and(|c| c.index == *index) {
                        self.checks[slot] = None;
                    }
                }
            }
        }
    }

    fn walk(&mut self, node: &'a Node<'a>) {
        let kind = NodeKind::of(node) as usize;
        if let Some(d) = self.disabled.as_deref_mut() {
            d.visit(node, self.root);
        }
        let dispatch = self.dispatch;
        for &slot in &dispatch.enter[kind] {
            let ancestors = std::mem::take(&mut self.ancestors);
            self.call(slot as usize, |c, cx| c.enter(node, &ancestors, cx));
            self.ancestors = ancestors;
        }
        for &slot in &dispatch.exit[kind] {
            let ancestors = std::mem::take(&mut self.ancestors);
            self.call(slot as usize, |c, cx| c.exit(node, &ancestors, cx));
            self.ancestors = ancestors;
        }
        self.sweep_failed();
        self.ancestors.push(node);
        for_each_visit_child(node, |c| self.walk(c));
        self.ancestors.pop();
    }
}

fn run_liquid_file<'a>(
    run: &RunContext<'a>,
    prepared: &[Prepared],
    dispatch: &LiquidDispatch,
    file: &'a SourceCode<'a>,
    file_index: u32,
) -> FileResult {
    let state = FileState::new(file_index);
    let n = dispatch.entries.len();
    let mut cxs: Vec<Option<LiquidCx<'a, '_>>> = Vec::with_capacity(n);
    let mut checks: Vec<Option<Box<dyn LiquidCheck<'a> + 'a>>> = Vec::with_capacity(n);
    for (pi, create) in &dispatch.entries {
        let p = &prepared[*pi];
        if is_ignored(&file.uri, run.config, Some(p.code)) {
            cxs.push(None);
            checks.push(None);
            continue;
        }
        let cx = Cx {
            run,
            file,
            def: p.def,
            index: p.index,
            settings: &p.settings,
            severity: p.severity,
            state: &state,
            _corrector: PhantomData,
        };
        checks.push(create(&cx));
        cxs.push(Some(cx));
    }
    let mut calls = vec![0u32; n];
    for slot in 0..n {
        if let (Some(check), Some(cx)) = (checks[slot].as_deref_mut(), cxs[slot].as_ref()) {
            state.tick.set(0);
            check.on_code_path_start(cx);
            calls[slot] = 1;
        }
    }

    let mut disabled = None;
    if let Ok(root) = file.liquid_ast() {
        // DisabledChecksVisitor's meta has no code: `config.settings[undefined]`.
        if !is_ignored(&file.uri, run.config, Some("undefined")) {
            disabled = Some(DisabledChecks::default());
        }
        {
            let mut walk = LiquidWalk {
                cxs: &cxs,
                checks: &mut checks,
                dispatch,
                ancestors: Vec::with_capacity(64),
                disabled: disabled.as_mut(),
                root,
                state: &state,
                calls: &mut calls,
            };
            walk.sweep_failed();
            walk.walk(root);
        }
        for slot in 0..n {
            if let (Some(check), Some(cx)) = (checks[slot].as_deref_mut(), cxs[slot].as_ref()) {
                state.tick.set(calls[slot]);
                check.on_code_path_end(cx);
            }
        }
    }
    finish_file(file, state, disabled)
}

fn finish_file<'a>(
    file: &'a SourceCode<'a>,
    state: FileState<'a>,
    disabled: Option<DisabledChecks>,
) -> FileResult {
    let mut offenses = state.offenses.into_inner();
    if let Some(d) = disabled {
        // `offenses.filter((offense) => !isDisabled(offense))`.
        offenses.retain(|o| !d.is_disabled(o));
    }
    FileResult { uri: file.uri.clone(), offenses, errors: state.failed.into_inner() }
}

fn run_json_file<'a>(
    run: &RunContext<'a>,
    prepared: &[Prepared],
    dispatch: &JsonTable,
    file: &'a SourceCode<'a>,
    file_index: u32,
) -> FileResult {
    let state = FileState::new(file_index);
    let n = dispatch.entries.len();
    let mut cxs: Vec<Option<JsonCx<'a, '_>>> = Vec::with_capacity(n);
    let mut checks: Vec<Option<Box<dyn JsonCheck<'a> + 'a>>> = Vec::with_capacity(n);
    for (pi, create) in &dispatch.entries {
        let p = &prepared[*pi];
        if is_ignored(&file.uri, run.config, Some(p.code)) {
            cxs.push(None);
            checks.push(None);
            continue;
        }
        let cx = Cx {
            run,
            file,
            def: p.def,
            index: p.index,
            settings: &p.settings,
            severity: p.severity,
            state: &state,
            _corrector: PhantomData,
        };
        checks.push(create(&cx));
        cxs.push(Some(cx));
    }
    let drop_failed = |checks: &mut Vec<Option<Box<dyn JsonCheck<'a> + 'a>>>, cxs: &[Option<JsonCx<'a, '_>>]| {
        if state.any_failed.get() {
            state.any_failed.set(false);
            for (index, _) in state.failed.borrow().iter() {
                for (slot, cx) in cxs.iter().enumerate() {
                    if cx.as_ref().is_some_and(|c| c.index == *index) {
                        checks[slot] = None;
                    }
                }
            }
        }
    };
    state.tick.set(0);
    for slot in 0..n {
        if let (Some(check), Some(cx)) = (checks[slot].as_deref_mut(), cxs[slot].as_ref()) {
            check.on_code_path_start(cx);
        }
    }
    drop_failed(&mut checks, &cxs);
    let mut calls = vec![1u32; n];
    if let Ast::Json(Ok(root)) = &file.ast {
        let any_visitor = (0..JsonKind::COUNT).any(|k| !dispatch.enter[k].is_empty() || !dispatch.exit[k].is_empty());
        if any_visitor {
            json_walk(root, &mut Vec::with_capacity(32), dispatch, &mut checks, &cxs, &state, &mut calls);
            drop_failed(&mut checks, &cxs);
        }
        for slot in 0..n {
            if let (Some(check), Some(cx)) = (checks[slot].as_deref_mut(), cxs[slot].as_ref()) {
                state.tick.set(calls[slot]);
                check.on_code_path_end(cx);
            }
        }
    }
    finish_file(file, state, None)
}

fn json_walk<'a>(
    node: &'a JsonNode<'a>,
    ancestors: &mut Vec<&'a JsonNode<'a>>,
    dispatch: &JsonTable,
    checks: &mut [Option<Box<dyn JsonCheck<'a> + 'a>>],
    cxs: &[Option<JsonCx<'a, '_>>],
    state: &FileState<'a>,
    calls: &mut [u32],
) {
    let kind = JsonKind::of(node) as usize;
    for &slot in &dispatch.enter[kind] {
        let slot = slot as usize;
        if let (Some(check), Some(cx)) = (checks[slot].as_deref_mut(), cxs[slot].as_ref()) {
            calls[slot] += 1;
            state.tick.set(calls[slot]);
            check.enter(node, ancestors, cx);
        }
    }
    for &slot in &dispatch.exit[kind] {
        let slot = slot as usize;
        if let (Some(check), Some(cx)) = (checks[slot].as_deref_mut(), cxs[slot].as_ref()) {
            calls[slot] += 1;
            state.tick.set(calls[slot]);
            check.exit(node, ancestors, cx);
        }
    }
    if state.any_failed.get() {
        state.any_failed.set(false);
        for (index, _) in state.failed.borrow().iter() {
            for (slot, cx) in cxs.iter().enumerate() {
                if cx.as_ref().is_some_and(|c| c.index == *index) {
                    checks[slot] = None;
                }
            }
        }
    }
    ancestors.push(node);
    for_each_json_child(node, |c| json_walk(c, ancestors, dispatch, checks, cxs, state, calls));
    ancestors.pop();
}
