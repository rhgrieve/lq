#!/usr/bin/env node
// Gate 2: attributes every `--mode embedded` diff, per theme and language. See fmt-attrib.mjs.
// Usage: fmt-classify.mjs [--no-mess] [--width N] [--show N] [--filter S] [--samples N] [--seed N]
import fs from 'node:fs';
import path from 'node:path';
import { GEN, THEMES, liquidFiles, messSource, oracleRequire, runLq, writeCases } from './lib.mjs';
import { attribute, firstDiff, mutateCases, printIssue } from './fmt-attrib.mjs';

const prettier = oracleRequire('prettier');
const plugin = oracleRequire('@shopify/prettier-plugin-liquid');
const args = process.argv.slice(2);
const val = (f, d) => (args.includes(f) ? args[args.indexOf(f) + 1] : d);
const width = Number(val('--width', 100));
const show = Number(val('--show', 5));
const samples = Number(val('--samples', 0));
const filter = val('--filter', null);

let cases = [];
for (const [theme, dir] of Object.entries(THEMES)) {
  for (const f of liquidFiles(dir)) {
    const src = fs.readFileSync(f, 'utf8');
    cases.push({ theme, label: f, src });
    if (!args.includes('--no-mess')) messSource(src).forEach((s, i) => cases.push({ theme, label: `${f}#mess${i}`, src: s }));
  }
}
if (val('--seed', null) !== null) cases = mutateCases(cases.filter((c) => !c.label.includes('#mess')), Number(val('--seed', 0)));
if (filter) cases = cases.filter((c) => c.label.includes(filter));

const opts = { parser: 'liquid-html', plugins: [plugin], printWidth: width };
const want = await Promise.all(cases.map(async (c) => {
  try { return { out: await prettier.format(c.src, opts) }; } catch (e) { return { error: String(e.message).split('\n')[0] }; }
}));
const files = writeCases(path.join(GEN, 'fmt-cases'), cases.map((c) => c.src));
const listFile = path.join(GEN, 'fmt-cases.list');
fs.writeFileSync(listFile, files.join('\n'));
const got = runLq(['debug-fmt', '--files-from', listFile, '--print-width', String(width), '--embedded', 'auto'])
  .trim().split('\n').map((l) => JSON.parse(l));

const rows = {};
const blank = () => ({ blocks: 0, identical: 0, biomeFormat: 0, biomeRejects: 0, prettierRejects: 0, passthrough: 0, integration: 0 });
const entries = [];
let identicalFiles = 0, differFiles = 0, bothError = 0;
const errorMismatch = [];
for (const [i, c] of cases.entries()) {
  const w = want[i], g = got[i];
  if (w.error !== undefined || g.error !== undefined) {
    if (w.error !== undefined && g.error !== undefined) bothError++;
    else errorMismatch.push(`${c.label}: oracle=${w.error} lq=${g.error}`);
    continue;
  }
  if (w.out === g.out) identicalFiles++; else differFiles++;
  entries.push({
    label: c.label, src: c.src, oracle: w.out, lq: g.out, options: { printWidth: width },
    count(kind, outcome) { const r = (rows[`${c.theme}/${kind}`] ??= blank()); r.blocks++; r[outcome]++; },
  });
}
const { outside, integration, examples } = await attribute(entries, 'fmt-classify');
for (const e of errorMismatch.slice(0, show)) console.log(`  error mismatch: ${e}`);
for (const e of [...outside, ...integration].slice(0, show)) printIssue(e);
for (const e of examples.slice(0, samples)) {
  const [x, y, n] = firstDiff(e.oracle, e.lq);
  console.log(`  biome-format ${e.e.label} (${e.kind}, line ${n})`);
  console.log(x.map((l) => `    prettier| ${l}`).join('\n'));
  console.log(y.map((l) => `    biome   | ${l}`).join('\n'));
}
console.table(rows);
const json = integration.filter((e) => e.kind === 'json').length;
console.log(`[fmt/classify/w${width}] files: ${identicalFiles + bothError}/${cases.length} identical (${bothError} both error), ${differFiles} differ; ` +
  `outside bodies: ${outside.length}; JSON body diffs: ${json}; integration errors: ${integration.length}; error mismatches: ${errorMismatch.length}`);
process.exit(outside.length || integration.length || errorMismatch.length ? 1 : 0);
