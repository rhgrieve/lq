#!/usr/bin/env node
// Compares `lq fmt` with Prettier + @shopify/prettier-plugin-liquid byte for byte.
//   exact      embeddedLanguageFormatting=off: Liquid/HTML printer only. Must be zero diffs.
//   embedded   default options: includes JSON (must match) and JS/CSS via Biome (reported).
// Usage: fmt.mjs [--mode exact|embedded] [--no-mess] [--limit N] [--show N] [--width N] [--filter S] [--seed N]
//   --seed N   replaces the corpus with seeded mutations of it (lib.mjs breakSource): tolerant-parse
//              edge cases. Errors must then agree, including the first line of the message.
import fs from 'node:fs';
import path from 'node:path';
import { GEN, THEMES, liquidFiles, messSource, oracleRequire, runLq, writeCases } from './lib.mjs';
import { mutateCases } from './fmt-attrib.mjs';

const prettier = oracleRequire('prettier');
const plugin = oracleRequire('@shopify/prettier-plugin-liquid');
const args = process.argv.slice(2);
const val = (f, d) => (args.includes(f) ? args[args.indexOf(f) + 1] : d);
const mode = val('--mode', 'exact');
const show = Number(val('--show', 5));
const width = Number(val('--width', 100));
const filter = val('--filter', null);
const embedded = mode !== 'exact';

const real = Object.values(THEMES).flatMap(liquidFiles);
let cases = real.map((f) => ({ label: f, src: fs.readFileSync(f, 'utf8') }));
const seed = val('--seed', null);
if (seed !== null) cases = mutateCases(cases, Number(seed));
if (seed !== null) {
  // Mutations replace the mess variants.
} else if (!args.includes('--no-mess')) {
  const mess = [];
  for (const c of cases) messSource(c.src).forEach((s, i) => mess.push({ label: `${c.label}#mess${i}`, src: s }));
  cases = cases.concat(mess);
}
if (filter) cases = cases.filter((c) => c.label.includes(filter));
cases = cases.slice(0, Number(val('--limit', cases.length)));

const opts = {
  parser: 'liquid-html',
  plugins: [plugin],
  printWidth: width,
  embeddedLanguageFormatting: embedded ? 'auto' : 'off',
};
const want = await Promise.all(
  cases.map(async (c) => {
    try {
      return { out: await prettier.format(c.src, opts) };
    } catch (e) {
      return { error: String(e.message).split('\n')[0] };
    }
  }),
);

const files = writeCases(path.join(GEN, 'fmt-cases'), cases.map((c) => c.src));
const listFile = path.join(GEN, 'fmt-cases.list');
fs.writeFileSync(listFile, files.join('\n'));
const lines = runLq([
  'debug-fmt', '--files-from', listFile, '--print-width', String(width),
  '--embedded', embedded ? 'auto' : 'off',
]).trim().split('\n');

let same = 0, diff = 0, bothErr = 0, msgDiff = 0, shown = 0;
const failures = [];
cases.forEach((c, i) => {
  const got = JSON.parse(lines[i]);
  const w = want[i];
  if (w.error !== undefined && got.error !== undefined) {
    bothErr++;
    if (w.error === got.error) { same++; return; }
    msgDiff++;
    diff++;
    failures.push(c.label);
    if (shown++ < show) console.log(`  ${c.label}\n    oracle| ${w.error}\n    lq    | ${got.error}`);
    return;
  }
  if (w.out === got.out) { same++; return; }
  diff++;
  failures.push(c.label);
  if (shown++ < show) {
    const a = (w.out ?? `<error: ${w.error}>`).split('\n');
    const b = (got.out ?? `<error: ${got.error}>`).split('\n');
    let n = 0;
    while (n < a.length && a[n] === b[n]) n++;
    console.log(`  ${c.label} (line ${n + 1})`);
    console.log(a.slice(Math.max(0, n - 1), n + 3).map((l) => `    oracle| ${l}`).join('\n'));
    console.log(b.slice(Math.max(0, n - 1), n + 3).map((l) => `    lq    | ${l}`).join('\n'));
  }
});
fs.writeFileSync(path.join(GEN, `fmt-failures-${mode}.txt`), failures.join('\n'));
console.log(`[fmt/${mode}/w${width}${seed !== null ? `/seed${seed}` : ''}] ${same}/${cases.length} match (${bothErr} both error, ${msgDiff} with different messages), ${diff} differ`);
process.exit(diff ? 1 : 0);
