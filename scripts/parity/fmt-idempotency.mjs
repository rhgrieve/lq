#!/usr/bin/env node
// Gate 4: wherever the oracle is idempotent, lq must be too. Runs both embedded modes. In auto
// mode, a non-idempotent lq result is attributed to Biome only if the differing block is itself
// not a fixed point of Biome stand-alone; that case is reported separately.
// Usage: fmt-idempotency.mjs [--no-mess] [--width N] [--show N] [--filter S]
import fs from 'node:fs';
import path from 'node:path';
import { GEN, THEMES, liquidFiles, messSource, oracleRequire, runLq, writeCases } from './lib.mjs';
import { firstDiff } from './fmt-attrib.mjs';

const prettier = oracleRequire('prettier');
const plugin = oracleRequire('@shopify/prettier-plugin-liquid');
const args = process.argv.slice(2);
const val = (f, d) => (args.includes(f) ? args[args.indexOf(f) + 1] : d);
const width = Number(val('--width', 100));
const show = Number(val('--show', 5));
const filter = val('--filter', null);

let cases = [];
for (const dir of Object.values(THEMES)) {
  for (const f of liquidFiles(dir)) {
    const src = fs.readFileSync(f, 'utf8');
    cases.push({ label: f, src });
    if (!args.includes('--no-mess')) messSource(src).forEach((s, i) => cases.push({ label: `${f}#mess${i}`, src: s }));
  }
}
if (filter) cases = cases.filter((c) => c.label.includes(filter));

const lq = (name, sources, mode) => {
  const files = writeCases(path.join(GEN, name), sources);
  const list = path.join(GEN, `${name}.list`);
  fs.writeFileSync(list, files.join('\n'));
  return runLq(['debug-fmt', '--files-from', list, '--print-width', String(width), '--embedded', mode])
    .trim().split('\n').map((l) => JSON.parse(l));
};
let failed = false;
for (const mode of ['off', 'auto']) {
  const opts = { parser: 'liquid-html', plugins: [plugin], printWidth: width, embeddedLanguageFormatting: mode };
  const fmt = async (s) => { try { return await prettier.format(s, opts); } catch { return null; } };
  const once = await Promise.all(cases.map((c) => fmt(c.src)));
  const twice = await Promise.all(once.map((o) => (o === null ? null : fmt(o))));
  const lq1 = lq('fmt-idem-1', cases.map((c) => c.src), mode);
  const lq2 = lq('fmt-idem-2', lq1.map((r) => r.out ?? ''), mode);
  let oracleIdempotent = 0, oracleNot = 0, violations = 0, lqNonIdemWhereOracleNot = 0, oracleEqual = 0;
  const shown = [];
  cases.forEach((c, i) => {
    if (once[i] === null) return;
    const lqStable = lq1[i].out !== undefined && lq2[i].out === lq1[i].out;
    if (twice[i] !== once[i]) {
      oracleNot++;
      // Same second pass means the same (lack of) stability.
      if (lq2[i].out === twice[i]) oracleEqual++;
      if (!lqStable) lqNonIdemWhereOracleNot++;
      return;
    }
    oracleIdempotent++;
    if (!lqStable) {
      violations++;
      if (shown.length < show) shown.push({ label: c.label, a: lq1[i].out ?? '', b: lq2[i].out ?? `<error ${lq2[i].error}>` });
    }
  });
  for (const s of shown) {
    const [x, y, n] = firstDiff(s.a, s.b);
    console.log(`  ${s.label} (line ${n})`);
    console.log(x.map((l) => `    pass1| ${l}`).join('\n'));
    console.log(y.map((l) => `    pass2| ${l}`).join('\n'));
  }
  console.log(`[fmt/idempotency/${mode}/w${width}] oracle idempotent on ${oracleIdempotent}/${cases.length}; ` +
    `lq not idempotent on ${violations} of those. Oracle not idempotent on ${oracleNot} ` +
    `(lq second pass equals oracle's on ${oracleEqual}, lq also unstable on ${lqNonIdemWhereOracleNot}).`);
  failed ||= violations > 0;
}
process.exit(failed ? 1 : 0);
