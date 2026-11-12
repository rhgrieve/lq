# Migrating a Shopify theme to lq

Instructions for a coding agent moving a theme repository from `shopify theme check` and Prettier
to lq. Work through the steps in order, verify, then report back to the user.

lq is a Rust port of `shopify theme check` (theme-check 3.31.0) and `@shopify/prettier-plugin-liquid`
(1.11.3). It reads the same config files and produces the same results, except where listed in
[DIVERGENCES.md](https://github.com/rhgrieve/lq/blob/main/DIVERGENCES.md) and the
[known gaps](https://github.com/rhgrieve/lq#known-gaps). It is not affiliated with Shopify.

## 1. Find what to change

Search the repository, excluding `node_modules`, for every place Theme Check or Prettier runs on
Liquid:

- `package.json` scripts
- Git hooks: `lefthook.yml`, `.husky/`, `.pre-commit-config.yaml`, lint-staged config
- CI: `.github/workflows/`, and any other CI config
- `Makefile`, shell scripts, editor settings in `.vscode/`
- Docs that tell people which commands to run: `README.md`, `CONTRIBUTING.md`

Useful patterns: `theme check`, `theme-check`, `prettier`, `prettier-plugin-liquid`.

Note whether Prettier also formats non-Liquid files (JS, CSS, JSON, Markdown). That decides step 4.

## 2. Install lq

If the repository has a `package.json`, add the release tarball as a dev dependency and install with
the repository's own package manager:

```json
{
  "devDependencies": {
    "lq": "https://github.com/rhgrieve/lq/releases/download/v0.2.0/lq-theme-check-0.2.0.tgz"
  }
}
```

Check https://github.com/rhgrieve/lq/releases for a newer version first. The tarball has binaries for
macOS and Linux (arm64, x64) and Windows (x64), no install script, and downloads nothing at run time.

Without a `package.json`, build from source with a Rust toolchain:

```sh
cargo install --git https://github.com/rhgrieve/lq lq-cli --locked
```

Confirm it runs: `npx lq --version` (or `lq --version`).

## 3. Replace Theme Check

| Before | After |
|---|---|
| `shopify theme check` | `lq check` |
| `shopify theme check --output json` | `lq check --output json` (identical JSON) |
| `shopify theme check --auto-correct` | `lq check --auto-correct` |
| `--path`, `--config`/`-C`, `--fail-level`, `--print`, `--list`, `--init` | same flags |

- Keep `.theme-check.yml`. lq reads it, including `extends`, `ignore`, `root`, `context` and
  per-check settings.
- If the config uses `require` for third-party checks, lq doesn't load them. Tell the user.
- `SHOPIFY_FLAG_*` environment variables and `--environment` aren't supported. Pass flags
  explicitly instead.
- lq's default text report has its own layout. If a script parses Theme Check's text output, switch
  it to `--output json`, or use `--output upstream` for upstream's text layout.
- In GitHub Actions, `lq check --output github` adds annotations to pull requests.
- Keep the Shopify CLI if the repository uses it for anything else, such as `theme dev` or
  `theme push`. Only replace the `theme check` calls.

## 4. Replace Prettier for Liquid

| Before | After |
|---|---|
| `prettier --check "**/*.liquid"` | `lq fmt "**/*.liquid" --check` |
| `prettier --write "**/*.liquid"` | `lq fmt "**/*.liquid" --write` |
| `prettier --list-different ...` | `lq fmt ... --list-different` |

- Keep the Prettier config. lq reads `.prettierrc` (JSON or YAML), `.prettierrc.json`,
  `.prettierrc.yml`, `.prettierrc.yaml`, `.prettierrc.json5`, the `prettier` key in `package.json`,
  overrides, `.editorconfig`, `.prettierignore` and `.gitignore`.
- A config in JS, TS or TOML (`prettier.config.js`, `.prettierrc.mjs`, `.prettierrc.toml`, ...)
  isn't supported. Convert it to `.prettierrc.json` with the same options.
- If Prettier only ever formatted Liquid, remove `prettier` and `@shopify/prettier-plugin-liquid`
  from the dev dependencies.
- If Prettier also formats other files, keep it for those, and change its commands or globs so they
  no longer include `.liquid` files. Don't add `*.liquid` to `.prettierignore`: lq reads that file
  too, and would skip every Liquid file.
- For hooks that pass staged files, add `--ignore-unknown` so non-Liquid files are skipped, or
  restrict the hook with a glob.

lefthook example:

```yaml
pre-commit:
  parallel: true
  commands:
    lint:
      glob: "*.{liquid,json}"
      run: npx lq check {staged_files}
    format:
      glob: "*.liquid"
      run: npx lq fmt --write {staged_files}
      stage_fixed: true
```

`lq all` runs `lq check` then `lq fmt --check`, and `lq all --write` applies safe fixes then formats.
Use it where one command is simpler.

## 5. Verify

Run these before changing anything else, and keep the output for the report.

1. **Lint.** Run `npx lq check --output json > lq.json`. If the Shopify CLI is available, run
   `shopify theme check --output json > upstream.json` and compare the offense sets (file, check,
   severity, range, message). Order can differ; the set shouldn't. Differences should be ones listed
   in DIVERGENCES.md, most often fewer false `OrphanedSnippet` offenses. `lq check --upstream-bugs`
   reproduces upstream exactly, which helps to confirm a difference is a known divergence.
2. **Formatting.** On a tree Prettier considered clean, run `npx lq fmt "**/*.liquid" --check`. If
   lq lists files, the difference is almost always in embedded `<style>` or `<script>` blocks, which
   lq formats with Biome instead of Prettier. Show the user the diff for one or two of those files
   (`npx lq fmt path/to/file.liquid | diff path/to/file.liquid -`). Don't reformat the theme as part
   of the migration without asking.
3. **Run what you changed.** Run each updated `package.json` script and hook command once, and check
   the exit codes: `lq check` exits 0 when clean, 1 for offenses at or above the fail level, 2 for
   bad usage; `lq fmt` exits 0 when clean, 1 when `--check` finds differences, 2 for errors.
4. Delete `lq.json` and `upstream.json` when done.

## 6. Report

Tell the user:

- Which files you changed, and each command before and after.
- What the comparison in step 5 found: matching results, or each difference and whether it's a
  documented divergence.
- Anything lq doesn't support that the repository used: third-party checks, JS/TS/TOML Prettier
  configs, `SHOPIFY_FLAG_*` variables.
- Any files `lq fmt --check` would change, and that formatting them is their call.

If something looks like a bug in lq, the user can report it at https://github.com/rhgrieve/lq/issues
with the smallest Liquid file that reproduces it.
