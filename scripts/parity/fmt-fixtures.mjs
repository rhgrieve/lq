#!/usr/bin/env node
// Gate 3: the plugin's src/test fixtures, split and configured as test-helpers.ts does, run through
// the oracle and lq. Each paragraph of index.liquid is a case, and so is each of fixed.liquid
// (formatting already formatted code). Remaining diffs must be attributed to Biome.
// Usage: fmt-fixtures.mjs [--show N] [--filter S] [--samples N]
import fs from 'node:fs';
import path from 'node:path';
import { GEN, ROOT, oracleRequire, runLq, writeCases } from './lib.mjs';
import { attribute, firstDiff, printIssue } from './fmt-attrib.mjs';

const prettier = oracleRequire('prettier');
const plugin = oracleRequire('@shopify/prettier-plugin-liquid');
const args = process.argv.slice(2);
const val = (f, d) => (args.includes(f) ? args[args.indexOf(f) + 1] : d);
const show = Number(val('--show', 10));
const samples = Number(val('--samples', 0));
const filter = val('--filter', null);
const TEST = path.join(ROOT, 'reference/theme-tools/packages/prettier-plugin-liquid/src/test');

// Copied from test-helpers.ts.
const PARAGRAPH_SPLITTER = /(?:\r?\n){2,}(?=\/\/|It|When|If|focus|debug|skip|<)/i;
const TEST_MESSAGE = /^(\/\/|It|When|If|focus|debug|skip)((\s|\S)(?!<)(?!{)(?!---))*./i;

function chunkOptions(chunk) {
  const m = TEST_MESSAGE.exec(chunk);
  const message = (m ? m[0] : '').replace(/^\/\/\s*/, '').replace(/\r?\n/g, ' ').trimEnd().replace(/\.$/, '');
  const options = { printWidth: 80, indentSchema: true, trailingComma: 'es5' };
  const parser = /(?<name>\w+): (?<value>[^\s]*)/g;
  let match;
  while ((match = parser.exec(message)) !== null) {
    try { options[match.groups.name] = JSON.parse(match.groups.value); } catch { /* upstream would throw */ return null; }
  }
  return { options, skip: /^skip/i.test(message) };
}

const cases = [];
for (const d of fs.readdirSync(TEST, { withFileTypes: true })) {
  if (!d.isDirectory() || (filter && !d.name.includes(filter))) continue;
  const dir = path.join(TEST, d.name);
  if (!fs.existsSync(path.join(dir, 'index.liquid'))) continue;
  const index = fs.readFileSync(path.join(dir, 'index.liquid'), 'utf8').split(PARAGRAPH_SPLITTER).map((s) => s.trimEnd());
  const fixed = fs.readFileSync(path.join(dir, 'fixed.liquid'), 'utf8').split(PARAGRAPH_SPLITTER).map((s) => s.trimEnd());
  const add = (label, chunk, optionsChunk, expected) => {
    const setup = chunkOptions(optionsChunk);
    if (!setup || setup.skip) return;
    cases.push({ label, src: chunk.replace(TEST_MESSAGE, '').trimStart(), options: setup.options, expected });
  };
  index.forEach((chunk, i) => {
    const expected = fixed[i] === undefined ? undefined : fixed[i].replace(TEST_MESSAGE, '').trimStart();
    add(`${d.name}/index#${i}`, chunk, chunk, expected);
    if (fixed[i] !== undefined) add(`${d.name}/fixed#${i}`, fixed[i], chunk);
  });
}

const want = await Promise.all(cases.map(async (c) => {
  try { return { out: await prettier.format(c.src, { parser: 'liquid-html', plugins: [plugin], ...c.options }) }; }
  catch (e) { return { error: String(e.message).split('\n')[0] }; }
}));
const files = writeCases(path.join(GEN, 'fmt-fixture-cases'), cases.map((c) => c.src));
const listFile = path.join(GEN, 'fmt-fixture-cases.list');
fs.writeFileSync(listFile, cases.map((c, i) => JSON.stringify({ file: files[i], options: c.options })).join('\n'));
const got = runLq(['debug-fmt', '--cases', listFile]).trim().split('\n').map((l) => JSON.parse(l));

let same = 0, bothErr = 0, oracleVsFixed = 0;
const errorMismatch = [];
const rows = {};
const entries = [];
for (const [i, c] of cases.entries()) {
  const w = want[i], g = got[i];
  if (w.out !== undefined && c.expected !== undefined && w.out.trimEnd() !== c.expected) oracleVsFixed++;
  if (w.error !== undefined || g.error !== undefined) {
    if (w.error !== undefined && g.error !== undefined) bothErr++;
    else errorMismatch.push(`${c.label}: oracle=${w.error} lq=${g.error}`);
    continue;
  }
  if (w.out === g.out) same++;
  entries.push({
    label: c.label, src: c.src, oracle: w.out, lq: g.out, options: c.options,
    count(kind, outcome) { const r = (rows[kind] ??= { blocks: 0, identical: 0, biomeFormat: 0, biomeRejects: 0, prettierRejects: 0, passthrough: 0, integration: 0 }); r.blocks++; r[outcome]++; },
  });
}
const { outside, integration, examples } = await attribute(entries, 'fmt-fixtures');
for (const e of errorMismatch.slice(0, show)) console.log(`  error mismatch: ${e}`);
for (const e of [...outside, ...integration].slice(0, show)) printIssue(e);
for (const e of examples.slice(0, samples)) {
  const [x, y, n] = firstDiff(e.oracle, e.lq);
  console.log(`  biome-format ${e.e.label} (${e.kind}, line ${n})`);
  console.log(x.map((l) => `    prettier| ${l}`).join('\n'));
  console.log(y.map((l) => `    biome   | ${l}`).join('\n'));
}
console.table(rows);
const differ = entries.length - same;
console.log(`[fmt/fixtures] ${same + bothErr}/${cases.length} match (${bothErr} both error), ${differ + errorMismatch.length} differ: ` +
  `${examples.length} Biome-format, ${rows.markdown?.passthrough ?? 0} markdown pass-through, ${Object.values(rows).reduce((t, r) => t + r.biomeRejects + r.prettierRejects, 0)} parser disagreement, ` +
  `${outside.length} outside bodies, ${integration.length} integration, ${errorMismatch.length} error mismatch; ` +
  `oracle vs fixed.liquid mismatches: ${oracleVsFixed}`);
process.exit(outside.length || integration.length || errorMismatch.length ? 1 : 0);
