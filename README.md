# lq

A fast native clone of `shopify theme check` and the Liquid Prettier plugin, in Rust. One binary, no runtime, and no
network.

`lq` is not affiliated with or endorsed by Shopify. It's a port of MIT-licensed code from
[Shopify/theme-tools](https://github.com/Shopify/theme-tools), and of the printing algorithm of
[Prettier](https://github.com/prettier/prettier) (MIT). See [NOTICE.md](NOTICE.md).

On Shopify's Horizon theme, `lq check` takes 0.054s against 3.4s for `shopify theme check`, and `lq fmt --check` takes
0.025s against 2.0s for Prettier: see [Benchmarks](#benchmarks).

## Install

From source, with a Rust toolchain:

```sh
cargo install --path crates/lq-cli --locked    # installs `lq`
# or
cargo build --release                 # target/release/lq
```

Embedded CSS, JS and TS are formatted with [Biome](https://github.com/biomejs/biome), a git dependency at a pinned
tag, so the first build needs network access and takes a few minutes. Nothing does afterwards.

### From a GitHub release

Each release attaches an npm tarball with binaries for macOS and Linux, on arm64 and x64, and for Windows on x64. Add it
to `package.json`:

```json
{
  "devDependencies": {
    "lq": "https://github.com/rhgrieve/lq/releases/download/v0.2.0/lq-theme-check-0.2.0.tgz"
  }
}
```

The repository must be public, because npm requests the file without GitHub credentials. There's no install script, and
nothing is downloaded at run time.

Node's startup is most of the run time on a small theme, so how you call it matters. On Dawn:

| how | `lq check` |
|---|---|
| the binary directly | 33ms |
| `node_modules/.bin/lq`, or `lq` in an npm script | 80ms |
| `npx lq` | 275ms |

To skip Node in a script, get the binary's path first:

```sh
"$(node_modules/.bin/lq --print-binary-path)" check
```

## Usage

```sh
lq check                              # lint the theme in the current directory
lq check --path themes/main           # lint a theme elsewhere
lq check snippets/a.liquid            # report only this file; the whole theme is still loaded
lq check --output json                # upstream's JSON on stdout
lq check --output short               # one line per offense, for grep, editors and agents
lq check --output github              # annotations for GitHub Actions
lq check --output json-rich           # the whole text report as data, hints included
lq check --output upstream            # upstream's text layout
lq explain MissingTemplate            # what a check does, and how to configure or disable it
lq check --only OrphanedSnippet       # report one check; see "Filtering"
lq check --severity error             # report errors only
lq check --fail-level warning         # exit 1 on warnings too
lq check --auto-correct               # apply safe fixes in place
lq check --timings                    # time per phase, on stderr

lq fmt "**/*.liquid" --check          # exit 1 if a file would change
lq fmt "**/*.liquid" --write          # rewrite files in place
lq fmt --stdin-filepath a.liquid      # stdin to stdout, using the config for that path

lq all                                # lq check, then lq fmt --check; see "Running both"
lq all --write                        # apply safe fixes, then format
```

`lq --help`, `lq check --help`, `lq fmt --help` and `lq all --help` list everything. `lq check` accepts upstream's offline flags. Its
JSON is upstream's, byte for byte. Its text report is its own: see [Reports](#reports). `lq fmt` behaves like `prettier`
for Liquid files.

File arguments to `lq check` narrow what is *reported*, and what decides the exit code. If other files have errors, a note on stderr says how many. The whole theme is still
loaded, so cross-file checks stay correct: a file's offenses are the same either way. Paths may be relative to the
current directory or to the theme root, a directory selects everything under it, and a path that doesn't exist exits 2.

### Filtering

```sh
lq check --only OrphanedSnippet                # one check
lq check --only orphan,asset                   # any check with that in its name, in any case
lq check --only 'Asset*' --skip AssetPreload   # * anchors the pattern; --skip wins over --only
lq check --severity warning snippets/          # errors and warnings, in these files
lq check --only DeprecatedFilter --auto-correct # fix one kind of problem
lq check --only asset --list                   # which enabled checks a filter selects
```

The report's summary lists the checks it found, to copy into `--only`. These flags are `lq`'s own, not upstream's.

Filters work like file arguments: every check still runs, and they narrow what is reported, what decides the exit code and
what `--auto-correct` touches. A filtered report is exactly a subset of the full one, in every output format. A note on
stderr says how much was left out, and a check turned off in the config gets a note rather than looking clean. A name that
matches no check exits 2 and suggests the closest one, so a typo can't pass as a clean theme.

A full name or alias selects just that check, so `ValidSchema` leaves out `ValidSchemaName`.

### Running both

```sh
lq all                         # lq check, then lq fmt --check on every .liquid file
lq all --write                 # lq check --auto-correct, then lq fmt --write
lq all snippets/a.liquid       # both, on these files or directories
lq all --output json           # check's JSON on stdout; fmt's output stays on stderr
```

Check runs first, because a fix can leave code that needs formatting. Both run even if check finds offenses, so one run
reports everything. The exit code is the higher of the two. If check exits 2, for bad usage or a path that doesn't exist,
fmt doesn't run.

`--write` is `lq all`'s only flag of its own. Every other `lq check` option works and applies to check only, except
`--path`, which also sets the directory fmt works in. `--auto-correct`, `--init`, `--list`, `--print` and `--version`
exit 2.

The two commands still decide separately what they cover. `ignore:` in `.theme-check.yml` doesn't affect fmt, and
`.prettierignore` doesn't affect check. `--only`, `--skip` and `--severity` don't narrow fmt. With no file arguments, fmt
covers every `.liquid` file under the theme root, while check covers the theme's own directories. File arguments can't be
globs, and fmt skips any that aren't Liquid. `--output github` has no annotations for unformatted files: they appear as
`[warn]` lines in the log and fail the step. For fmt's own options, run `lq fmt`.

### lefthook

```yaml
pre-commit:
  parallel: true
  commands:
    lint:
      glob: "*.{liquid,json}"
      run: lq check {staged_files}
    format:
      glob: "*.liquid"
      run: lq fmt --write {staged_files}
      stage_fixed: true
```

Without a `glob`, add `--ignore-unknown` to `lq fmt`, so that other files are skipped rather than errors, as with
Prettier. `lq check` ignores files that aren't in the theme.

As one command, which also applies `lq check`'s safe fixes:

```yaml
pre-commit:
  commands:
    lq:
      glob: "*.{liquid,json}"
      run: lq all --write {staged_files}
      stage_fixed: true
```

### Reports

The default report is for reading in a terminal. Each diagnostic has a clickable `path:line:col`, the code around it with
the problem underlined, and what to do next:

```
warning UnrecognizedRenderSnippetArguments
Unknown argument 'subtitel' in render tag for snippet 'card'.
  ╭─ sections/main.liquid:4:31
  │
3 │ {% render 'hero' %}
4 │ {% render 'card', title: 'a', subtitel: 'b' %}
  ·                               ━━━━━━━━━━━━━ unknown argument
5 │ {% render 'footer' %}
  │
  ├ note did you mean 'subtitle'?
  ├ note snippets/card.liquid declares title, subtitle
  ├ try  Remove 'subtitel'
  │      - {% render 'card', title: 'a', subtitel: 'b' %}
  │      + {% render 'card', title: 'a' %}
  ╰ docs lq explain UnrecognizedRenderSnippetArguments
```

`fix` is a change `--auto-correct` applies. `try` is a suggestion it leaves to you. Repeats of the same problem are
folded under `also`, one location per line, and `--no-fold` prints each in full. The summary at the end says what was
found, and why the run passed or failed.

Color follows `--color auto|always|never`, `NO_COLOR`, `FORCE_COLOR` and `CLICOLOR_FORCE`, and the width follows `COLUMNS`
or the terminal. Text from theme files can't send escape sequences to your terminal.

### CI

```yaml
- run: npm ci # with lq in package.json, as above
- run: npx lq check --output github # annotations on the pull request
- run: npx lq fmt "**/*.liquid" --check
```

Or, as one step, `npx lq all --output github`.

GitHub limits how many annotations it displays per step, so on a theme with many offenses keep the full report as well,
with `--output json` or `--output json-rich`.

Exit codes: `lq check` gives 0 when clean, 1 for offenses at or above the fail level or a bad config, and 2 for bad
usage. `lq fmt` gives 0 when clean, 1 when `--check` or `--list-different` found differences, and 2 for errors. `lq all`
gives the higher of the two.

### Config files

- `lq check`: `.theme-check.yml` in the theme root, or `--config`, with `extends`, `root`, `ignore`, `context`, `require`
  (kept, not loaded) and per-check settings, plus the built-in `theme-check:all`, `:recommended`,
  `:theme-app-extension` and `:nothing`.
- `lq fmt`: `.prettierrc` (JSON or YAML), `.prettierrc.json`, `.prettierrc.yml`, `.prettierrc.yaml`,
  `.prettierrc.json5`, the `prettier` key of `package.json` or `package.yaml`, overrides, `.editorconfig`,
  `.prettierignore` and `.gitignore`.

## Parity

The claim, exactly:

- **Pinned versions.** `@shopify/theme-check-node` 3.31.0 with `theme-check-common` and `theme-graph` 3.31.0,
  `@shopify/liquid-html-parser` 2.11.0, `@shopify/prettier-plugin-liquid` 1.11.3 and Prettier 3.9.10. Docs and JSON
  schemas are a snapshot of `theme-liquid-docs` in `data/docs`.
- **Lint.** All 91 check definitions are implemented. `lq check --upstream-bugs`, with all checks on, has zero
  differences from upstream's `themeCheckRun` on the offense *set* (file, check, severity, range, message) on three
  real themes: Dawn (`3e9ee22`), Horizon (`9960490`) and a private production theme. Set `LQ_EXTRA_THEME=<dir>` to add
  a theme of your own to every harness.
- **Specs.** 5093 of 5098 records from upstream's own spec suite replay identically, including fixes and suggestions.
  The other 5 can't be replayed, because they mock the remote asset size request. None fail.
- **Formatter.** With embedded formatting off, output is byte-identical to Prettier on the same themes, including
  badly formatted variants of them, at widths 100, 80 and 40. On upstream's fixtures, 887 of 888 cases match with
  embedded formatting on, the other being the Markdown block below. JSON blocks go through a native port and are
  byte-identical. JS, TS and CSS go through Biome, and differ: see below. `lq fmt --check` has the
  same output and exit code as `prettier --check`, config resolution and ignore files included.
- **Default mode** differs from all of this only as listed in [DIVERGENCES.md](DIVERGENCES.md).

Not covered: anything outside the known gaps below, naturally, and code unlike these three themes and upstream's
fixtures. A difference found on other code is a bug worth reporting.

### Running the harnesses

The harnesses compare against the real upstream, installed by hand in the gitignored `reference/`:

```sh
mkdir -p reference/oracle && cd reference/oracle
npm install @shopify/theme-check-node@3.31.0 @shopify/prettier-plugin-liquid@1.11.3 prettier@3.9.10
cd ../.. && git clone --depth 1 --branch @shopify/theme-check-node@3.31.0 https://github.com/Shopify/theme-tools reference/theme-tools
git clone --depth 1 https://github.com/Shopify/dawn reference/corpus/dawn
git clone --depth 1 https://github.com/Shopify/horizon reference/corpus/horizon
```

Then, with `cargo build --release --workspace`:

| Harness | Compares |
|---|---|
| `cargo test --workspace` | unit tests, and the spec goldens in `--upstream-bugs` mode |
| `node scripts/parity/goldens.mjs` | the spec goldens, per check |
| `node scripts/parity/lint.mjs` | `lq check --upstream-bugs` with upstream, on the three themes |
| `node scripts/parity/lint.mjs --default-mode` | the default with upstream: every difference must have a divergence |
| `node scripts/parity/divergences.mjs` | each divergence: upstream's behaviour, `--upstream-bugs`, and the fix |
| `node scripts/parity/autofix.mjs` | `--auto-correct` on disk: the files left behind match upstream, plus the `AutofixAbort` fix |
| `node scripts/parity/engine.mjs` | the theme graph, doc definitions, CSS classes and block parameters |
| `node scripts/parity/liquid-syntax.mjs` | `LiquidSyntaxError`, differentially |
| `node scripts/parity/ast.mjs` | the parser's AST |
| `node scripts/parity/visit.mjs` | the check visitor's order |
| `node scripts/parity/jsonschema.mjs` | the JSON schema validator |
| `node scripts/parity/config.mjs`, `discovery.mjs` | config resolution, YAML, file discovery, ignores |
| `node scripts/parity/fmt.mjs --width 100` (also 80, 40) | the printer, byte for byte, with embedded formatting off |
| `node scripts/parity/fmt-classify.mjs --no-mess` | attributes every embedded difference to Biome or a bug of ours |
| `node scripts/parity/fmt-fixtures.mjs` | the formatter on upstream's fixtures |
| `node scripts/parity/fmt-config.mjs` | `lq fmt --check` with `prettier --check`, on a real theme |
| `node scripts/parity/fmt-biome-guard.mjs` | a Biome panic falls back to verbatim, on the release binary |

### Unseen themes

As a check against overfitting to the themes used during development, these were compared once after the port was
finished, on 2026-11-05: Dawn v2.0.0, v8.0.0, v12.0.0 and v15.0.0, and Shopify's skeleton theme. Lint offense sets were
identical (230, 13, 16, 16 and 0 offenses, with `--upstream-bugs`). Formatting was identical on all 2,370 files,
including messy variants: print widths 100 and 80 with embedded formatting off, and width 100 with it on. On the private
production theme, the real `shopify theme check` from CLI 5.0.1 and `lq check --upstream-bugs` report the same offense set,
counts and exit codes.

## Known gaps

- **JS and CSS use Biome, not Prettier.** At width 100, Dawn and the private production theme differ in 0 blocks and Horizon
  in 32 of 156 CSS blocks. At width 80, the private production theme differs in 5 of 45 CSS blocks, counting real and
  messy variants. The causes are `calc()`, `:not()` and multi-value properties. So `lq fmt --check` can fail on CSS
  that Prettier accepts, and `--write` would then reformat it: on Horizon, `lq` flags the 28 files Prettier flags plus
  14 more.
- **Markdown blocks** pass through verbatim.
- **`lq fmt` doesn't report syntax errors,** as with Prettier: it reformats a file with a broken tag, which can rearrange the code around it. In a hook, run `lq check` before `lq fmt --write`.
- **Extreme nesting is slow.** A file with 100,000 nested `{% if %}`s takes about 56s to lint, because checks read a node's ancestors. Real themes aren't close to this: 6,960 ordinary files take 0.5s.
- **Parser disagreements** on code outside the corpus: where Biome rejects what Prettier's parsers accept, such as
  top-level `return`, `|>`, `//` in CSS, or `{{ }}` in CSS `url()`, the block prints verbatim.
- **JS, TS and TOML Prettier configs** are unsupported, and reported as errors.
- **Offense order.** File order and cross-check order within a file can differ from upstream's. Set equality is the
  guarantee: upstream's own order varied between runs.
- **Remote asset sizes** are never looked up, since there's no network: the remote path of `AssetSize*` doesn't report.
- **IDNA** handling in URL validation is partial.
- **Docs** are pinned to a snapshot instead of being updated at runtime.
- **Third-party checks** through `require` in a config are unsupported.
- **`SHOPIFY_FLAG_*` environment variables and `--environment`** are unsupported.

## Divergences

Upstream has bugs, and `lq` fixes the ones with a demonstrated reproduction. 20 are listed in
[DIVERGENCES.md](DIVERGENCES.md), each with its reproduction and a test against the real upstream. `lq check
--upstream-bugs` reproduces upstream for all but one: a string `$schema` crashes upstream's whole process, which isn't
worth reproducing.

The biggest in practice: a JSON template section with no `type`, which is a legitimate context override, makes
upstream's graph fail silently and reports every snippet as orphaned. On the private production theme, default mode removes
118 false `OrphanedSnippet` offenses of 119. That is the only difference from upstream on that theme, and Dawn and
Horizon have none.

`lq`'s text report is also a deliberate difference, of another kind: see
[DIVERGENCES.md](DIVERGENCES.md#presentation). `--upstream-bugs` or `--output upstream` prints upstream's layout.

## Benchmarks

Apple M3 Max, 14 cores, 36 GB, Darwin 25.1.0, 2026-11-05. Horizon at `9960490` (282 `.liquid` files, 383 theme files),
11 runs each, `lq` built with `cargo build --release`, `shopify` CLI 5.0.1, Node 22.13.0, Prettier 3.9.10.

| command | median | min | max |
|---|---|---|---|
| `lq check --output json` | 0.054s | 0.053s | 0.075s |
| `shopify theme check --output=json` | 3.380s | 3.349s | 3.402s |
| `lq fmt "**/*.liquid" --check` | 0.025s | 0.024s | 0.040s |
| `prettier --check "**/*.liquid"` | 1.956s | 1.913s | 2.124s |

`lq check` is 63x faster and `lq fmt --check` 78x faster.

Re-run with `scripts/bench.sh [--runs N] [theme]`. It's read-only, needs nothing beyond bash and what it measures, and
runs a fresh process each time. "Cold" means no daemon or cache of ours: the OS file cache stays warm.

## Releasing

1. Set the version in `Cargo.toml`'s `[workspace.package]`, and commit it with the updated `Cargo.lock`.
2. Tag and push: `git tag v0.2.0 && git push origin main v0.2.0`.

`.github/workflows/release.yml` then builds all five platforms and runs the tests on each, packs one tarball with
`scripts/pack-npm.mjs`, installs and runs that tarball on each platform, and attaches it to a new GitHub release. The tag
must match `Cargo.toml`, or packing fails. Linux binaries are static (musl), so they don't depend on the host's glibc, and
Windows binaries link the C runtime statically, so they don't need the Visual C++ redistributable. To try all of this
without releasing, run the workflow by hand: `gh workflow run release.yml --ref <branch>`.

To build a tarball locally for this machine only, put the binary at `<vendor>/<platform>-<arch>/lq` (`lq.exe` on Windows), then run
`node scripts/pack-npm.mjs --vendor <vendor> --out dist --upstream <dir> --allow-missing`. `<dir>` holds
`theme-tools/LICENSE.md` and `prettier/LICENSE`.

## Re-pinning to a new upstream release

1. Reinstall `reference/oracle` and re-clone `reference/theme-tools` at the new versions.
2. Update `data/docs` from `theme-liquid-docs`, and `NOTICE.md` and `THEME_CHECK_VERSION` in
   `crates/lq-cli/src/check_cmd.rs`.
3. `node scripts/parity/gen-registry.mjs` regenerates the check registry and `reference/gen/registry.json`. New checks
   come up as not implemented; `crates/lq-check/PORTING.md` is the guide to porting one.
4. `scripts/parity/record-goldens.sh` re-records the specs into `tests/goldens/`.
5. Diff upstream between the tags, port what changed, and run every harness above until they're at zero.
6. For each entry in `DIVERGENCES.md`, run `divergences.mjs`: a case failing on "upstream isn't the reported behaviour"
   means upstream changed. If upstream fixed it, remove the variant and its branches.

## License

MIT. See [NOTICE.md](NOTICE.md) for upstream attribution.
