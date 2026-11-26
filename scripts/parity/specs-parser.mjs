#!/usr/bin/env node
// Runs upstream's parser specs (`specs/parser/*.yml` in Shopify/theme-tools) against `lq debug-ast`,
// the way `packages/liquid-html-parser/src/specs.test.ts` does: the whole tree, positions as
// `{ start: [line, character], text }`, without `source` and the links between nodes.
//
//   node scripts/parity/specs-parser.mjs <theme-tools>/specs [--show N] [--mode check|fmt] [file.yml...]
//
// `--mode` picks lq's parser target: `check` (theme-tools main, the default) or `fmt` (the
// liquid-html-parser that Prettier plugin 1.11.3 uses).
import fs from 'node:fs';
import path from 'node:path';
import { GEN, oracleRequire, runLq } from './lib.mjs';

const YAML = oracleRequire('yaml');
const args = process.argv.slice(2);
const opt = (f, d) => (args.includes(f) ? args[args.indexOf(f) + 1] : d);
const show = Number(opt('--show', 20));
const mode = opt('--mode', 'check');
const specs = args[0];
const only = args.slice(1).filter((a, i, all) => a.endsWith('.yml') && !['--show', '--mode'].includes(all[i - 1]));
const DROP = new Set(['source', '_source', 'parentNode', 'prev', 'next', 'firstChild', 'lastChild']);

const isRange = (v) => v && typeof v.start === 'number' && typeof v.end === 'number' && Object.keys(v).length === 2;
const position = (source, start, end) => {
  const before = source.slice(0, start);
  return { start: [before.split('\n').length - 1, start - (before.lastIndexOf('\n') + 1)], text: source.slice(start, end) };
};
const toSpecTree = (source, tree) =>
  JSON.parse(JSON.stringify(tree, (k, v) => (DROP.has(k) ? undefined : isRange(v) ? position(source, v.start, v.end) : v)));
const toSpecError = (source, e) => {
  const lines = source.split('\n');
  const offset = ({ line, column }) => lines.slice(0, line - 1).reduce((n, l) => n + l.length + 1, 0) + column - 1;
  const out = { message: e.message, ...(e.loc ? position(source, offset(e.loc.start), offset(e.loc.end)) : {}) };
  if (e.unclosed) {
    const { type, name, blockStartPosition: b } = e.unclosed;
    out.unclosed = { type, name, blockStartPosition: position(source, b.start, b.end) };
  }
  return out;
};
// Deep equality where property order doesn't matter and list order does.
const canon = (v) =>
  Array.isArray(v) ? v.map(canon) : v && typeof v === 'object' ? Object.fromEntries(Object.keys(v).sort().map((k) => [k, canon(v[k])])) : v;
const firstDiff = (a, b, at = '') => {
  if (JSON.stringify(canon(a)) === JSON.stringify(canon(b))) return null;
  if (a && b && typeof a === 'object' && typeof b === 'object') {
    for (const k of new Set([...Object.keys(a), ...Object.keys(b)])) {
      const d = firstDiff(a[k], b[k], `${at}.${k}`);
      if (d) return d;
    }
  }
  return `${at}: want ${JSON.stringify(a)?.slice(0, 300)} got ${JSON.stringify(b)?.slice(0, 300)}`;
};

const dir = path.join(GEN, 'specs-parser');
fs.rmSync(dir, { recursive: true, force: true });
fs.mkdirSync(dir, { recursive: true });
const runs = { html: [], liquid: [] };
const cases = [];
for (const file of fs.readdirSync(path.join(specs, 'parser')).filter((f) => f.endsWith('.yml')).sort()) {
  if (only.length && !only.includes(file)) continue;
  const spec = YAML.parse(fs.readFileSync(path.join(specs, 'parser', file), 'utf8'));
  for (const c of spec.cases) {
    const id = cases.length;
    const p = path.join(dir, `${id}.liquid`);
    fs.writeFileSync(p, c.source);
    cases.push({ file, c, p });
    for (const parse of c.parse) runs[parse].push(id);
  }
}
const results = new Map();
for (const [parse, ids] of Object.entries(runs)) {
  if (!ids.length) continue;
  const flags = parse === 'liquid' ? ['--liquid-only'] : [];
  const list = path.join(dir, `${parse}.txt`);
  fs.writeFileSync(list, ids.map((id) => cases[id].p).join('\n'));
  const out = runLq(['debug-ast', ...flags, '--parser', mode, '--files-from', list]).split('\n');
  ids.forEach((id, i) => results.set(`${parse}:${id}`, JSON.parse(out[i])));
}
let passed = 0;
const failures = [];
const byFile = {};
cases.forEach(({ file, c }, id) => {
  const want = c.error ? { error: c.error } : { ast: c.ast };
  let problem = null;
  for (const parse of c.parse) {
    const r = results.get(`${parse}:${id}`);
    const got = r.error ? { error: toSpecError(c.source, r.error) } : { ast: toSpecTree(c.source, r.ast) };
    const d = firstDiff(want, got);
    if (d) {
      problem = `${parse} ${d}`;
      break;
    }
  }
  byFile[file] ??= [0, 0];
  if (problem) {
    byFile[file][1]++;
    failures.push(`${file}: ${c.name}\n    ${problem}`);
  } else {
    passed++;
    byFile[file][0]++;
  }
});
for (const f of failures.slice(0, show)) console.log(`FAIL ${f}`);
for (const [f, [p, n]] of Object.entries(byFile)) if (n) console.log(`  ${f}: ${p} passed, ${n} failed`);
console.log(`[specs-parser:${mode}] passed ${passed}, failed ${failures.length}`);
process.exit(failures.length ? 1 : 0);
