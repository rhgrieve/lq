# Porting a check

Upstream source is at `reference/theme-tools-main/packages/theme-check-common/src/checks/<dir>/index.ts`
(Shopify/theme-tools `main` at `af2e1a78`, the commit `lq check` targets; `reference/` is gitignored scratch). Every check already has a file,
`crates/lq-check/src/checks/<snake_case_export_name>.rs`, holding
`pub const IMPL: CheckImpl = CheckImpl::NotImplemented;`. Replace it with the port. Edit only that file, plus
`checks/shared.rs` for helpers several checks need, so agents working in parallel don't collide. Metadata (code,
severity, settings schema, aliases, targets) is already in `registry.rs`; don't repeat it.

## The shape of a check

```rust
use crate::api::*;
use lq_syntax::Node;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidFilter]), // the handlers upstream defines
    exit: NodeMask::NONE,                           // `X:exit` handlers
};

fn create<'a, 'f>(_cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    Some(Box::new(MyCheck)) // `None` is upstream's `return {}`
}

struct MyCheck;

impl<'a> LiquidCheck<'a> for MyCheck {
    fn enter(&mut self, node: &'a Node<'a>, ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidFilter(f) = node else { return };
        let p = node.position();
        cx.report(LiquidProblem::new(format!("no {}", f.name), p.start, p.end)
            .suggest("Remove it", |c| c.remove(p.start, p.end)));
    }
    // on_code_path_start, exit, on_code_path_end as needed.
}
```

JSON checks are the same with `CheckImpl::Json`, `JsonCheck`, `JsonCx`, `JsonProblem`, `JsonMask`, `JsonKind`.
Read `checks/unused_assign.rs` (stateful Liquid), `checks/matching_translations.rs` (cross-file JSON, fix and
suggestion) and `checks/missing_template.rs` (file system, settings).

One instance is created per file and check, so struct fields are per-file state, and `create` runs before any
visit. All Liquid checks on a file share one walk: **a handler runs only for kinds in the masks**, and a kind
missing from the mask silently never fires. List exactly the kinds your `match` handles. Keep `create` and the
handlers cheap: they run for every file, including files the check doesn't care about, so bail in `create`
when you can.

## What `cx` gives you (`api.rs`)

| Upstream | Here |
|---|---|
| `context.report({...})` | `cx.report(LiquidProblem::new(msg, start, end).fix(..).suggest(msg, ..))` |
| `context.file` / `.uri` / `.source` / `.ast` | `cx.file()`, `cx.uri()`, `cx.source()`, `cx.file().liquid_ast()` or `.json_ast()` |
| `context.settings.foo` | `cx.settings().str("foo")`, `.f64`, `.bool`, `.strs`, `.get` |
| `context.toUri`, `toRelativePath`, `mode` | `cx.to_uri`, `cx.to_relative_path`, `cx.mode()` |
| `fileExists`, `fileSize`, `doesFileExist` | `cx.file_exists(uri)`, `cx.file_size(uri)`, `cx.does_file_exist(rel)` |
| `getDefaultLocale`, `getDefaultTranslations` (+ schema) | `cx.default_locale()`, `cx.default_translations()`, `cx.default_schema_locale()`, `cx.default_schema_translations()` |
| `getBlockSchema`, `getSectionSchema`, `getAppBlockSchema`, `getSchema(context)` | `cx.block_schema(n)`, `cx.section_schema(n)`, `cx.app_block_schema(n)`, `cx.schema()` |
| `schema.validSchema` guard | `schema.valid_value()` gives the `Value` when valid, `schema.valid_schema()` the full result |
| `getDocDefinition`, `getBlockParameters` | `cx.doc_definition(rel)`, `cx.block_parameters(name)` |
| `getReferences`, `getDependencies`, `getCSSClassesForURI` | `cx.references(uri)`, `cx.dependencies(uri)`, `cx.css_classes_for_uri(uri)`, with `cx.has_*()` for `if (!context.x)` |
| `themeDocset` | `cx.docset()`: `.filters()`, `.objects()`, `.liquid_drops()`, `.tags()`, indexed `.filter(n)`, `.object(n)`, `.tag(n)`, `.has_system_translation(k)`. Only fields checks read are embedded; to add one, extend `lq-theme/build.rs` and `docs.rs` |
| `validateJSON` | `cx.has_validate_json()`, `cx.validate_json(uri, json)` (`Result`; byte offsets into `json`) |
| a rejected promise | `cx.fail(JsError { .. })`, stops this check on this file, keeps prior reports |

Helpers: `check_utils` (`checks/utils.ts`, `tags/content-for.ts`), `visitor::visit` (`visit()`), `visible_if`,
`liquid_doc` and `liquid_doc::arguments`, `utils::block`, `utils::file`, `utils::styles`, `utils::traversal`,
`to_schema`, `block_parameters`, `schema_settings`, `lq_json::ast::node_at_path`,
`lq_theme::minimatch::minimatch`, `lq_syntax::js` (`trim`, `slice`, ...), `lq_json::js`.

There's no async. Await points become plain calls; lazily computed things are cached across files, so call them
freely. Don't compute anything expensive before you know you need it.

## Parity traps specific to this codebase

- **Positions are UTF-8 bytes.** `node.position()`, JSON `loc()`, fix offsets and `Problem` start/end are all bytes,
  converted to UTF-16 only when reported. Never mix them with `.chars().count()`. But a *value* upstream measures
  (`str.length` in a message or threshold) is UTF-16: use `utils::js_len`. Add a schema's `offset` to JSON `loc`s.
- **`exit` isn't post-order.** Upstream's `X:exit` runs right after `X`, before the children. A node's fields are
  visited in reverse key order (a tag's `children` before its `markup`; `Property` value before key), array items
  in order. The walker already does this (`scripts/parity/visit.mjs` verifies it). Don't assume document order.
- **Ancestors** are the path from the root, excluding the node. `ancestors.last()` is `ancestors.at(-1)`.
- **The AST is one enum.** `node.type` is the `Node` variant; `AttrSingleQuoted`, `AttrDoubleQuoted` and
  `AttrUnquoted` share `AttrNode`. `typeof tag.markup === 'string'` is `Markup::Str`. Absent fields are `Option`.
- **JS whitespace.** `.trim()` and `\s` are `lq_syntax::js::trim` and `lq_json::js::is_js_whitespace`: U+FEFF is in,
  U+0085 out. Rust's `trim` and `is_whitespace` differ. `\w`, `\d` and `\b` are ASCII.
- **Regexes.** For anything more than a literal scan, `lq_json::regex::JsRegex` has JS semantics (flags, lookaround,
  captures, `$` not before a final newline) and is slow, so compile once in a `OnceLock` or hand-write hot scans.
- **Truthiness and dynamic values.** `validSchema`, `parsed` and translations are `lq_json::Value`. `if (x)` is
  `is_truthy()`: `""`, `0` and `null` are falsy. `'k' in obj` is `obj.get("k").is_some()`, even for a `null` value.
  `x?.y ?? z` treats null and absent alike. `typeof null` is `object`. `Object.keys(null)` throws: use `cx.fail`.
- **Key order.** `Object` has JS order: integer-like keys ascending first, then insertion order. `Map`/`Set` keep
  insertion order, and a re-set key keeps its slot: don't use `HashMap` for what you iterate, see `UnusedAssign`.
- **Messages are exact**, including quotes and punctuation. Copy them from upstream, not from memory. Numbers in
  messages go through `lq_json::js::number_to_string`, `JSON.stringify` through `lq_json::js::quote` or `Value::to_json`.
- **URIs.** Use `lq_json::uri` (`normalize`, `join`, `relative`, `basename`, `dirname`) or `cx.to_uri`, never string
  surgery: URIs here are `toString(true)`, so spaces aren't encoded but `#` and `?` are.
- **Fixes run at report time** and only record data. Fixers can't read mutable state later than that. A JSON fix
  re-serializes the whole file on apply, as upstream.
- **Promise.all ordering.** Where upstream fans out with `Promise.all`, report in the order its mock-fs timing
  produces. The specs compare offenses as a set; `lint.mjs` reports a different order within a file.
- `remoteFileSize`: **no network, ever.** `lq`'s own runs answer 0, which is upstream's behaviour when the request
  fails. The specs' mocks give real sizes.
- Two definitions share the code `MaxFileSize` (Liquid and JSON files), with separate files `max_file_size.rs` and
  `max_file_size_json.rs`. `UniqueSettingIds` has code `UniqueSettingId`.

## Upstream's bugs

A port reproduces upstream, bugs included. Where upstream has a demonstrated bug that's worth fixing, add a variant to
`enum Divergence` (`lq-theme/src/divergence.rs`), branch on `cx.divergence(Divergence::X)`, add a case to
`scripts/parity/divergences.mjs` and a row to `DIVERGENCES.md`. The fix is on by default, and every parity gate runs
with `--upstream-bugs`, which turns them all off. Don't fix a bug any other way: a gate must always be able to compare
with upstream.

## Specs

`specs/upstream/checks/<check>.yml` holds upstream's language-neutral specs for each check: the theme, docset,
references, settings and mode of each case, with its expected offenses, fixes and suggestions. They run on the mock
dependencies of upstream's test helper, in `--upstream-bugs` mode, and every case must pass.

```sh
cargo test -p lq-check --test upstream_specs -- --nocapture                       # every case, failures listed
LQ_SPECS_DEFAULT_MODE=1 cargo test -p lq-check --test upstream_specs -- --nocapture   # with lq's fixes on
```

Specs can't cover what upstream doesn't test, so also run the real thing:

```sh
LQ_PROFILE=fast node scripts/parity/lint.mjs --show 20     # lq vs upstream on the three themes
```

## Done means

1. `upstream_specs` passes every case of the check's spec. Don't special-case a test to pass; if something can't
   match, leave the failure visible and explain it.
2. `lint.mjs` shows no missing or extra offense for your check in any of the three themes. Its failures are listed
   per check; a difference in an implemented check makes it exit 1.
3. `cargo clippy -p lq-check --all-targets` is clean and no new dependency was added without asking.
4. Nothing slow runs for a file the check doesn't apply to: no `format!` on hot paths, no regex compiled per file,
   no `Vec` built per visited node.
