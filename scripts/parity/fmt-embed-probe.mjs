#!/usr/bin/env node
// Per-block comparison of Prettier's embedded parsers with Biome, across all themes:
// which raw blocks each accepts, and why Prettier rejects what Biome accepts.
// Usage: fmt-embed-probe.mjs [--dump N] [--width N]
import fs from 'node:fs';
import path from 'node:path';
import { GEN, THEMES, liquidFiles, oracleRequire, runLq } from './lib.mjs';

const prettier = oracleRequire('prettier');
const { toTolerantLiquidHtmlAST, walk } = oracleRequire('@shopify/liquid-html-parser');
const args = process.argv.slice(2);
const val = (f, d) => (args.includes(f) ? args[args.indexOf(f) + 1] : d);
const dump = Number(val('--dump', 5));
const width = Number(val('--width', 100));
const PARSER = { css: 'css', javascript: 'babel', typescript: 'typescript', json: 'json', markdown: 'markdown' };

const blocks = [];
for (const [theme, dir] of Object.entries(THEMES)) {
  for (const file of liquidFiles(dir)) {
    let ast;
    try { ast = toTolerantLiquidHtmlAST(fs.readFileSync(file, 'utf8')); } catch { continue; }
    walk(ast, (n) => {
      if (n.type === 'RawMarkup' && PARSER[n.kind] && n.value.trim() !== '') blocks.push({ theme, file, kind: n.kind, value: n.value });
    });
  }
}
const dir = path.join(GEN, 'fmt-embed-blocks');
fs.rmSync(dir, { recursive: true, force: true });
fs.mkdirSync(dir, { recursive: true });
const list = blocks.map((b, i) => { b.path = path.join(dir, `${i}.txt`); fs.writeFileSync(b.path, b.value); return JSON.stringify({ file: b.path, kind: b.kind, width }); });
const listFile = path.join(GEN, 'fmt-embed-blocks.list');
fs.writeFileSync(listFile, list.join('\n'));
const lq = runLq(['debug-embed', listFile]).trim().split('\n').map((l) => JSON.parse(l));
const table = {};
const failures = {};
for (const [i, b] of blocks.entries()) {
  let oracle;
  try {
    oracle = { out: (await prettier.format(b.value, { parser: PARSER[b.kind], printWidth: width, singleQuote: true, __embeddedInHtml: true })).trimEnd() };
  } catch (e) { oracle = { error: String(e.message).split('\n')[0] }; }
  const got = b.kind === 'json' || b.kind === 'markdown' ? { skip: true } : lq[i];
  const key = `${b.theme}/${b.kind}`;
  const row = (table[key] ??= { blocks: 0, bothOk: 0, same: 0, prettierErrOnly: 0, biomeErrOnly: 0, bothErr: 0, notBiome: 0 });
  row.blocks++;
  if (got.skip) { row.notBiome++; if (oracle.error) (failures[b.kind] ??= []).push({ b, error: oracle.error }); continue; }
  if (oracle.error && got.error) { row.bothErr++; (failures[`both/${b.kind}`] ??= []).push({ b, error: oracle.error }); }
  else if (oracle.error) { row.prettierErrOnly++; (failures[`prettierOnly/${b.kind}`] ??= []).push({ b, error: oracle.error }); }
  else if (got.error) { row.biomeErrOnly++; (failures[`biomeOnly/${b.kind}`] ??= []).push({ b, error: 'biome' }); }
  else { row.bothOk++; if (oracle.out === got.out) row.same++; }
}
console.table(table);
for (const [k, list] of Object.entries(failures)) {
  console.log(`== ${k}: ${list.length}`);
  const hist = {};
  for (const f of list) hist[f.error.replace(/\(\d+:\d+\)/, '')] = (hist[f.error.replace(/\(\d+:\d+\)/, '')] ?? 0) + 1;
  for (const [m, n] of Object.entries(hist).sort((a, b) => b[1] - a[1]).slice(0, 12)) console.log(`   ${n}x ${m}`);
  for (const f of list.slice(0, dump)) {
    console.log(`  - ${path.relative(path.dirname(path.dirname(f.b.file)), f.b.file)}: ${f.error}\n${f.b.value.trim().split('\n').slice(0, 4).map((l) => `      ${l.slice(0, 110)}`).join('\n')}`);
  }
}
