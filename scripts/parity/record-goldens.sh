#!/usr/bin/env bash
# Rebuilds tests/goldens/ from upstream's theme-check-common vitest specs.
#
# Applies record-goldens.patch to reference/theme-tools (idempotent, and the
# patch is a no-op unless LQ_GOLDEN_DIR is set), runs vitest with recording on,
# then merges, dedupes, and groups the records. The checkout stays patched.
#
# LQ_GOLDEN_VITEST_JSON=<path> also saves vitest's per-test JSON report.
set -euo pipefail

PARITY="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$PARITY/../.." && pwd)"
UPSTREAM="$ROOT/reference/theme-tools"
GOLDENS="$ROOT/tests/goldens"
PATCH="$PARITY/record-goldens.patch"

cd "$UPSTREAM"
[ -d node_modules ] || pnpm install --frozen-lockfile --ignore-scripts
# The specs resolve @shopify/liquid-html-parser through its package.json main.
[ -f packages/liquid-html-parser/dist/index.js ] ||
  pnpm --filter @shopify/liquid-html-parser build:ts

if git apply -R --check "$PATCH" 2>/dev/null; then
  echo "patch already applied"
else
  git apply "$PATCH"
  echo "patch applied"
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
REPORT="${LQ_GOLDEN_VITEST_JSON:-$WORK/vitest.json}"
LQ_GOLDEN_DIR="$WORK/records" pnpm vitest run packages/theme-check-common \
  --reporter=dot --reporter=json --outputFile.json="$REPORT"

node --input-type=module - "$WORK/records" "$GOLDENS" "$UPSTREAM" <<'EOF'
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const [recordsDir, goldens, upstream] = process.argv.slice(2);
const RLE_MIN_LENGTH = 64 * 1024;
const RLE_MIN_RUN = 64;
const SCHEMA_REF_MIN_LENGTH = 1024;

// Maps a check code to the checks/ subdirectory that defines it.
function checkDirs() {
  const checksDir = path.join(upstream, 'packages/theme-check-common/src/checks');
  const dirs = new Map();
  for (const entry of fs.readdirSync(checksDir, { recursive: true })) {
    if (!entry.endsWith('.ts') || entry.endsWith('.spec.ts')) continue;
    const source = fs.readFileSync(path.join(checksDir, entry), 'utf8');
    for (const m of source.matchAll(/\bcode:\s*['"](\w+)['"]/g)) {
      dirs.set(m[1], entry.split(path.sep)[0]);
    }
  }
  return dirs;
}

function targetFile(record, dirs) {
  const { checks, spec } = record;
  if (checks.length === 0 || record.unreplayable?.includes('custom check definition')) {
    return '_multi';
  }
  if (checks.length === 1) return checks[0];
  const specDir = /\/checks\/([^/]+)\//.exec(spec)?.[1];
  return checks.every((code) => dirs.get(code) === specDir) ? checks[0] : '_multi';
}

// Only huge filler sources (MaxFileSize) get this encoding.
function rle(source) {
  if (typeof source !== 'string' || source.length < RLE_MIN_LENGTH) return source;
  const parts = [];
  let literal = '';
  for (let i = 0; i < source.length; ) {
    let end = i + 1;
    while (end < source.length && source[end] === source[i]) end++;
    if (end - i >= RLE_MIN_RUN) {
      if (literal) parts.push(literal);
      literal = '';
      parts.push([source[i], end - i]);
    } else {
      literal += source.slice(i, end);
    }
    i = end;
  }
  if (literal) parts.push(literal);
  return { $rle: parts };
}

function compactTheme(theme) {
  for (const key of Object.keys(theme)) theme[key] = rle(theme[key]);
}

function externalizeSchemas(record, schemasDir) {
  const modes = record.deps.jsonValidationSet?.schemas;
  if (!modes) return;
  for (const list of Object.values(modes)) {
    if (!Array.isArray(list)) continue;
    for (const def of list) {
      if (typeof def.schema !== 'string' || def.schema.length < SCHEMA_REF_MIN_LENGTH) continue;
      const sha = createHash('sha1').update(def.schema).digest('hex');
      fs.mkdirSync(schemasDir, { recursive: true });
      fs.writeFileSync(path.join(schemasDir, `${sha}.json`), def.schema);
      delete def.schema;
      def.schemaRef = sha;
    }
  }
}

const lines = fs
  .readdirSync(recordsDir)
  .sort()
  .flatMap((f) => fs.readFileSync(path.join(recordsDir, f), 'utf8').split('\n').filter(Boolean));
const records = lines.map((line) => JSON.parse(line));
const bySpec = records
  .map((record, i) => [record, i])
  .sort(([a, i], [b, j]) => (a.spec < b.spec ? -1 : a.spec > b.spec ? 1 : i - j))
  .map(([record]) => record);

// Identical inputs collapse. Inputs with a different outcome stay: a vi.mock or
// an in-spec check definition can differ invisibly to the record.
const seen = new Map();
const unique = [];
let keptConflicts = 0;
for (const record of bySpec) {
  const { theme, checks, settings, mode, deps, offenses, fixed, error } = record;
  const input = JSON.stringify({ theme, checks, settings, mode, deps });
  const output = JSON.stringify({ offenses, fixed, error });
  const outputs = seen.get(input);
  if (outputs?.has(output)) continue;
  if (outputs) keptConflicts++;
  seen.set(input, (outputs ?? new Set()).add(output));
  unique.push(record);
}

fs.mkdirSync(goldens, { recursive: true });
for (const f of fs.readdirSync(goldens)) {
  if (f.endsWith('.jsonl') || f === '_schemas') {
    fs.rmSync(path.join(goldens, f), { recursive: true, force: true });
  }
}

const dirs = checkDirs();
const grouped = new Map();
for (const record of unique) {
  compactTheme(record.theme);
  if (record.deps.fs?.theme) compactTheme(record.deps.fs.theme);
  externalizeSchemas(record, path.join(goldens, '_schemas'));
  const file = targetFile(record, dirs);
  const line = JSON.stringify(record);
  if (/\/Users\/|\/home\//.test(line)) console.warn(`absolute path in: ${record.test}`);
  if (/\\ud[89ab][0-9a-f]{2}(?!\\ud[c-f])|(?<!\\ud[89ab][0-9a-f]{2})\\ud[c-f][0-9a-f]{2}/i.test(line)) {
    console.warn(`lone surrogate in: ${record.test}`);
  }
  if (!grouped.has(file)) grouped.set(file, { lines: [], unreplayable: new Map() });
  const group = grouped.get(file);
  group.lines.push(line);
  if (record.unreplayable) {
    const n = group.unreplayable.get(record.unreplayable) ?? 0;
    group.unreplayable.set(record.unreplayable, n + 1);
  }
}

for (const file of [...grouped.keys()].sort()) {
  const { lines: out, unreplayable } = grouped.get(file);
  fs.writeFileSync(path.join(goldens, `${file}.jsonl`), out.join('\n') + '\n');
  const reasons = [...unreplayable].map(([r, n]) => `${n}: ${r}`).join(' | ');
  console.log(`${file}\t${out.length}${reasons ? `\t${reasons}` : ''}`);
}
console.log(
  `recorded ${records.length}, kept ${unique.length}, ` +
    `same-input-different-output kept ${keptConflicts}, files ${grouped.size}`,
);
EOF
du -sh "$GOLDENS"
