# Divergences from upstream

`lq` targets `shopify theme check` (theme-check 3.31.0). Where upstream has a demonstrated bug, `lq` fixes it by
default. `lq check --upstream-bugs` turns every fix off and reproduces upstream, which is the mode all parity gates
run in.

Each entry is a variant of `enum Divergence` in `crates/lq-theme/src/divergence.rs`, read by checks through
`cx.divergence(Divergence::X)`. `lq debug-divergences` lists them.

**Proof.** `node scripts/parity/divergences.mjs` runs every reproduction below against the real upstream in
`reference/oracle`, and proves for each: upstream's behaviour is the bug; `--upstream-bugs` matches upstream on all
offenses of all checks, including what their fixes and suggestions produce; the default is the fix; and the default
leaves every other check as upstream has it. It accepts ids to run a subset. The "Test" column names its cases.

**On real themes.** `node scripts/parity/lint.mjs --default-mode` compares the default with upstream and files each
difference under the divergence causing it, found by running each fix alone (`--only-divergence <ID>`). Anything
unexplained fails.

| ID | Check | Minimal reproduction | Upstream | `lq` | Test |
|---|---|---|---|---|---|
| `GraphNoType` | theme graph (`OrphanedSnippet`, `ValidScopedCSSClass`) | `templates/index.context.b2b.json`: `{"sections":{"main":{"settings":{}}},"order":["main"]}`; a template block without `type` too | `traverseJsonModule` throws, the whole graph is lost, every snippet is orphaned | Skips such entries before creating a module; only unreferenced snippets are orphaned. These are legitimate context override templates | "a context override template section has no type", "a template block has no type" |
| `GraphFailure` | `OrphanedSnippet`, `ValidScopedCSSClass` | any remaining graph failure, e.g. `templates/bad.json`: `{}` | A failed graph looks empty: every snippet is orphaned and classes from parents look out of scope | No graph-based conclusions, and a warning on stderr | "a template with no sections fails the graph...", "a failed graph: a class from a parent..." |
| `UnclosedHtmlIdentifier` | `UnclosedHTMLElement` | `{% if a.b.c %}<a>{% endif %}` | ``condition `a['b'],['c']` `` | ``condition `a['b']['c']` `` | "two lookups" |
| `HtmlSyntaxQuotedLiteral` | `LiquidHTMLSyntaxError` | `{% if "it's" foo %}{% endif %}` | No report, while `{% if "its" foo %}{% endif %}` gets one, because `['"][^'"]*['"]` rejects the inner quote | Reported at 0:6-0:16. The pattern is `"[^"]*"\|'[^']*'`, so `'a"` is no longer a literal | "a literal holding the other quote", "control: without the quote..." |
| `LiquidSyntaxUnclosedLine` | `LiquidSyntaxError` | `{% liquid\n  block 'hero'\n%}`; `{% liquid\n block 'a'\n endblock\n block 'b'\n%}`; `partial` likewise | Reported on the whole tag: 0:0-2:2, 0:0-4:2 | On the unclosed statement: 1:2-1:14, 3:1-3:10 | "unclosed block", "the second block is the unclosed one", "unclosed partial" |
| `ContentForPrototype` | `ValidContentForArguments` | `{% content_for '__proto__' %}\n{% content_for 'block' %}` | `validationStrategies['__proto__']` finds `Object.prototype` and throws, losing the check's reports for the file | Own properties only; the second tag gets its two reports | "__proto__ stops the check for the file" |
| `AddArgumentSuggestion` | `MissingRenderSnippetArguments`, `MissingContentForArguments`, `MissingBlockArguments` | `{% render 'typed', %}`, `{% render 'typed'%}`, with `@param {object} title`, `@param {string} body` | `{% render 'typed',, title:  %}`, `{% render 'typed',, body: '' %}`; and inside the quotes: `{% render 'typed, body: '''%}` | `{% render 'typed', title: title %}`, `{% render 'typed', body: '' %}`, `{% render 'typed', body: ''%}`. See below for the value | "double comma, and an empty value...", "no space before the close..." |
| `PresetsBlockOrderPath` | `SchemaPresetsBlockOrder` | A preset's blocks `a`, with nested blocks, then a sibling `b` | `ast_path` is never popped: the sibling reports on `a`'s `block_order`, or `nodeAtPath` finds nothing and the check throws | Popped after recursing: the top-level `block_order`, or `b` itself | "a sibling after nested blocks reports on the wrong node", "...throws, losing the report" |
| `VisibleIfMatchIndex` | `ValidVisibleIf` | `"visible_if": "pre {{ section.settings.nope }}"`, also in `config/settings_schema.json` | Positions ignore `match.index`, 4 columns early here | Adds `match.index` | "text before the expression, in a section", "...in settings_schema.json" |
| `PrototypeLookup` | `ValidVisibleIf`, `ValidSchemaTranslations`, `ValidSchemaName` | `{{ section.settings.toString }}`; `"label": "t:toString"` | `refers to a namespace`; no offense | `was not found`; an offense. `Object.hasOwn` at each step, so keys named `length` or `404` still resolve (Horizon's `t:settings.length`, `t:names.404`: no change) | "validateLookup finds Object.prototype", "deepGet finds Object.prototype", "control: own keys..." |
| `SchemaDollarCrash` | engine | `{% schema %}{"$schema":"https://example.test/s.json","name":"a"}{% endschema %}` | An unhandled rejection in `vscode-json-languageservice` kills the whole process, with no output | Continues. **`--upstream-bugs` can't sensibly abort, so it also continues**: this is the one divergence it doesn't reproduce | "a string $schema kills upstream..." |
| `NestedBlockPath` | `JSONMissingBlock` | `{"sections":{"blocks":{"type":"x","blocks":{"k":{"type":"b"}}}}}` | `isNestedBlock` counts the section's key, so it reads `blocks/x.liquid`'s schema and misses the offense that any other key gets | Counts structural segments only | "a section keyed blocks looks nested", "control: any other key..." |
| `LinkToNoArgument` | `HardcodedRoutes` | `{{ 'x' \| link_to }}{{ 'Cart' \| link_to: '/cart' }}` | `node.args[0].type` throws, losing later reports in the file | Skips that filter; `/cart` is reported | "link_to with no argument loses later reports" |
| `EmptySchema` | `TranslationKeyExists`, `PaginationSize`, `AssetSizeAppBlockCSS`, `AssetSizeAppBlockJavaScript` | `{% schema %}{% endschema %}` or `{% schema %}null{% endschema %}` before anything else | `parseJSON` returns undefined or null, `isError` misses it, the next read throws and later reports are lost | Treated as no schema | "empty schema, ...", "null schema, ..." |
| `NullInPath` | `TranslationKeyExists`, `PaginationSize` | `locales/en.default.json`: `{"a":null}` with `{{ 'a.c' \| t }}{{ 'y' \| t }}`; `"settings":[null, ...]` | Throws on the null, losing every report for the file | Null is not found: both keys are reported; the settings item is skipped | "a null along a translation key", "a null settings item" |
| `EmptyAssetString` | `AssetSizeCSS`, `AssetSizeJavaScript` | `{{ '' \| stylesheet_tag }}`, `{{ '' \| script_tag }}` before a real asset | `value[0].length` throws, losing later reports | Skips that filter | "an empty string before stylesheet_tag", "...before script_tag" |
| `CdnPreconnectHost` | `CdnPreconnect` | `href="https://cdn.shopify.com"`; `href="https://xcdnXshopify.comy/a"`; `href="https://acdn.shopify.com.evil/"` | Misses the first, because `.+` is required after the host; flags the others, because dots are unescaped and nothing is anchored | Compares the lowercased hostname of an `http:`, `https:` or `//` URL | "missed without a path, wrong on lookalike hosts" |
| `RouteBoundary` | `HardcodedRoutes` | `<a href="/cartoons">`, `<a href="/collections/allx">`, `{{ 'x' \| link_to: '/cartoons' }}` | Prefix match: `/cart`, `/collections/all`, `/cart` | The route must end at the end of the text, `/`, `?` or `#`: none, `/collections` (as for any collection page), none | "routes match by prefix" |
| `ImgUrlPrototype` | `DeprecatedFilter` | `{{ p \| product_img_url: 'toString' }}`, `{{ p \| img_tag: 'a', 'b', 'toString' }}` | `size in NAMED_SIZES` finds `Object.prototype`: `image_url: width: function toString() { [native code] }, ...` | Own properties only: `image_url: width: 100` | "product_img_url: ...", "img_tag: ...", "control: img_url..." |
| `AutofixAbort` | engine | `lq check --auto-correct` on a theme with a locale starting `/* ... */` that has a missing translation, plus any other fixable offense | `JSON.parse` in the JSON corrector rejects the comment, and the run stops: no fix is applied after that point, even in unrelated Liquid files. `shopify theme check --auto-correct` therefore fixes nothing on such themes | The locale is skipped with a warning on stderr, since rewriting it would drop the comment header, and every other fix is applied | `scripts/parity/autofix.mjs` |

## Presentation

The table above is bugs. The default text report is a different kind of difference: a choice about how offenses are
presented, with no upstream bug behind it.

**The rule: which offenses are found, and upstream's JSON, stay identical. Only human-facing presentation diverges.**

| Surface | Status |
|---|---|
| `--output json` | Upstream's format, unchanged: no new fields, even for `lq`'s hints |
| `--output upstream`, or `--upstream-bugs` with no `--output` | Upstream's text layout, unchanged |
| `--output text`, the default | `lq`'s report, with snippets, hints, fix previews and folding |
| `--output short`, `github`, `json-rich`, `lq explain` | `lq`'s own, with no upstream equivalent |
| Offenses, messages, positions, fixes, suggestions, exit codes | Not presentation: the same in every format |
| `lq fmt --check` summary | Prettier's, except that the hint says ``Run `lq fmt` with --write to fix.``, because that's the command the reader has. `fmt-config.mjs` allows that one substitution |

Hints are set with `Problem::label`, `note` and `note_at`, and travel in `Offense.hints`, apart from everything the
goldens and `lint.mjs` compare. They're not a `Divergence`, because no check behaves differently, so `divergences.mjs`
has nothing to prove about them.

**Proof.** The parity gates all read `--output json` and don't change. `crates/lq-cli/tests/report.rs` pins the rest: every
location in the JSON appears in `text`, `short`, `github` and `json-rich`, on the fixture and on the reference themes, so
folding can't hide an offense; the exit code is the same in every format at every `--fail-level`; and the JSON carries
none of `lq`'s fields.

## Notes on individual fixes

**`AddArgumentSuggestion`, the value.** Upstream's value for a type with no literal default (`object`, a drop, or no
type) is empty. Its output, `{% render 'x', body:  %}`, is a `Syntax error in 'render' tag` by upstream's own check, so
it's clearly invalid, even though upstream's specs pin it. No literal is valid: `nil` fails
`ValidRenderSnippetArgumentTypes`. `lq` suggests `body: body` instead. It parses and type-checks, passing a same-named
variable is the common idiom, and `UndefinedObject` flags it if there's no such variable. Types with a literal default
keep upstream's `''`, `0`, `false` or first literal. The no-space case wasn't in the original report: it's the same
insertion at `end - 1`, found by the reproduction.

**`ImgUrlPrototype`.** The originally reported reproduction, `{{ p | img_url: 'toString' }}`, doesn't show the bug:
`img_url` scales the size first, which turns the function into `NaN`, and upstream suggests `width: 100`. The same
lookup does print the function through `product_img_url`, `article_img_url`, `collection_img_url` and `img_tag`, so the
fix applies there, and a control case pins the `img_url` result.

**`LiquidSyntaxUnclosedLine`.** Limited to `block` and `partial`, as upstream's own `liquidLineTagLocation` is. The
same reason for other tags, `{% liquid\n if x\n%}`, still reports the whole tag.

## Upstream's specs that the fixes contradict

Upstream's recorded specs replay in `--upstream-bugs` mode. In default mode four of them differ, by design, and
`crates/lq-check/tests/goldens.rs` pins that list. Two are `LiquidSyntaxUnclosedLine`, where the check reports the whole
tag although the unit test of upstream's `liquidLineTagLocation` beside them expects the statement. Two are
`AddArgumentSuggestion`'s empty value. `node scripts/parity/goldens.mjs --default-mode --show 10` prints them.

## Looks odd, kept

These look wrong at first but have no demonstrated bug, or a weak case, so `lq` matches upstream:

- `for ... limit: [0]` is treated differently from `tablerow`.
- `{% content_for 'blocks' @ %}`-style markup.
- `script_tag` with a CDN URL.
- `maxcdn` URLs reporting twice.

Upstream's non-deterministic offense order isn't chased either: see the README.
