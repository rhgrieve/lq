#!/usr/bin/env node
// Gate 5: `lq fmt --check` against `prettier --check` on a real theme, with its own
// .prettierrc.json and .prettierignore, in Liquid scope. Read-only: neither is passed --write, and
// Prettier gets an explicit --cache-location so it does not touch the theme's node_modules/.cache.
// Usage: fmt-config.mjs [--theme DIR] [--temp DIR]
//   --temp DIR   additionally runs config cases built under DIR: overrides, ignore rules, editorconfig.
import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { ORACLE, THEMES, lqBin, oracleRequire } from './lib.mjs';

const args = process.argv.slice(2);
const val = (f, d) => (args.includes(f) ? args[args.indexOf(f) + 1] : d);
const theme = val('--theme', THEMES.extra ?? THEMES.dawn);
const PRETTIER = path.join(ORACLE, 'node_modules/prettier/bin/prettier.cjs');
const PLUGIN = oracleRequire.resolve('@shopify/prettier-plugin-liquid');
const CACHE = path.join(os.tmpdir(), 'lq-config-cache');
const lq = path.resolve(lqBin());

const run = (bin, argv, cwd) => {
  const r = spawnSync(bin, argv, { cwd, encoding: 'utf8', maxBuffer: 1 << 28 });
  return { code: r.status, out: r.stdout, err: r.stderr };
};
const oracle = (cwd, patterns) => run('node', [PRETTIER, '--check', '--cache-location', CACHE, '--plugin', PLUGIN, ...patterns], cwd);
const ours = (cwd, patterns) => run(lq, ['fmt', ...patterns, '--check'], cwd);
// Prettier prints a stack for errors without a location: the first line is the comparable part.
const noStack = (r) => ({ ...r, err: r.err.split('\n').filter((l) => !/^\[error\]\s+at /.test(l)).join('\n') });
// lq names its own command in the --check hint: see Presentation in DIVERGENCES.md.
const ownHint = (r) => ({ ...r, err: r.err.replace('Run Prettier with --write to fix.', 'Run `lq fmt` with --write to fix.') });
const compare = (label, a0, b0) => {
  const a = ownHint(noStack(a0)), b = noStack(b0);
  const same = a.code === b.code && a.out === b.out && a.err === b.err;
  console.log(`[fmt/config] ${label}: oracle exit ${a.code}, lq exit ${b.code}; stdout ${a.out === b.out ? 'same' : 'DIFFERS'}, stderr ${a.err === b.err ? 'same' : 'DIFFERS'}`);
  if (!same) {
    console.log(`  oracle stdout: ${JSON.stringify(a.out.slice(0, 600))}\n  lq     stdout: ${JSON.stringify(b.out.slice(0, 600))}`);
    console.log(`  oracle stderr: ${JSON.stringify(a.err.slice(0, 1200))}\n  lq     stderr: ${JSON.stringify(b.err.slice(0, 1200))}`);
  } else {
    console.log(`  stdout: ${JSON.stringify(a.out)}\n  stderr: ${JSON.stringify(a.err.slice(0, 400))}`);
  }
  return same;
};

let ok = compare(`${path.basename(theme)} "**/*.liquid"`, oracle(theme, ['**/*.liquid']), ours(theme, ['**/*.liquid']));

// A messy project exercising overrides, ignore files and .editorconfig.
const temp = val('--temp', null);
if (temp) {
  fs.rmSync(temp, { recursive: true, force: true });
  const write = (p, c) => { const f = path.join(temp, p); fs.mkdirSync(path.dirname(f), { recursive: true }); fs.writeFileSync(f, c); };
  const messy = '<div   class="a">{{a|b}}</div>\n<script>var a = "b"</script>\n';
  const long = `<div>${'aaaaaaaaaa '.repeat(12)}</div>\n`;
  write('.prettierrc.json', JSON.stringify({ printWidth: 40, singleQuote: true, liquidSingleQuote: false, overrides: [
    { files: 'wide/*.liquid', options: { printWidth: 200 } },
    { files: '*.tabs.liquid', excludeFiles: 'sub/*.tabs.liquid', options: { useTabs: true } },
    { files: ['**/embed-off/**'], options: { embeddedLanguageFormatting: 'off' } },
  ] }));
  write('.prettierignore', '/ignored\n*.skip.liquid\nkeep/**\n!keep/yes.liquid\nweird dir/\n');
  write('.gitignore', 'gitignored/\n');
  write('.editorconfig', 'root = true\n[*]\nindent_style = space\nindent_size = 4\n[sub/**]\nindent_size = 8\nmax_line_length = 60\n');
  for (const p of ['a.liquid', 'wide/a.liquid', 'x.tabs.liquid', 'sub/x.tabs.liquid', 'embed-off/a.liquid', 'ignored/a.liquid', 'b.skip.liquid', 'keep/yes.liquid', 'keep/no.liquid', 'gitignored/a.liquid', 'node_modules/a.liquid', 'weird dir/a.liquid', 'UP.LIQUID', 'sub/deep/z.liquid', '.hidden/a.liquid']) {
    write(p, p === 'wide/a.liquid' ? long : messy);
  }
  write('formatted.liquid', '<div></div>\n');
  write('wide/fmt.liquid', long);
  write('nested/.prettierrc', 'printWidth: 60\ntabWidth: 3\noverrides:\n  - files: "*.liquid"\n    options:\n      singleAttributePerLine: true\n');
  write('nested/a.liquid', '<div a="1" b="2"><p>x</p></div>\n');
  write('pkg/package.json', JSON.stringify({ name: 'x', prettier: { printWidth: 30 } }));
  write('pkg/a.liquid', messy);
  write('broken.liquid', '{% toString %}\n');
  ok = compare('temp "**/*.liquid"', oracle(temp, ['**/*.liquid']), ours(temp, ['**/*.liquid'])) && ok;
  ok = compare('temp "." style dir sub', oracle(temp, ['sub/**/*.liquid', 'wide/fmt.liquid', 'formatted.liquid']), ours(temp, ['sub/**/*.liquid', 'wide/fmt.liquid', 'formatted.liquid'])) && ok;
  ok = compare('temp explicit ignored file', oracle(temp, ['ignored/a.liquid', 'formatted.liquid']), ours(temp, ['ignored/a.liquid', 'formatted.liquid'])) && ok;
  ok = compare('temp no match', oracle(temp, ['nope/**/*.liquid']), ours(temp, ['nope/**/*.liquid'])) && ok;
  ok = compare('temp clean', oracle(temp, ['formatted.liquid']), ours(temp, ['formatted.liquid'])) && ok;
  ok = compare('temp negated', oracle(temp, ['**/*.liquid', '!sub/**', '!broken.liquid']), ours(temp, ['**/*.liquid', '!sub/**', '!broken.liquid'])) && ok;
}
if (temp) {
  // Resolved options, through the formatted output of each file and of stdin.
  const files = [];
  const walk = (d) => { for (const e of fs.readdirSync(d, { withFileTypes: true })) { const p = path.join(d, e.name); if (e.isDirectory()) walk(p); else if (e.name.endsWith('.liquid')) files.push(path.relative(temp, p)); } };
  walk(temp);
  let same = 0;
  for (const f of files.sort()) {
    const a = noStack(run('node', [PRETTIER, '--cache-location', CACHE, '--plugin', PLUGIN, f], temp));
    const b = noStack(run(lq, ['fmt', f], temp));
    const input = fs.readFileSync(path.join(temp, f), 'utf8');
    const sa = spawnSync('node', [PRETTIER, '--cache-location', CACHE, '--plugin', PLUGIN, '--stdin-filepath', f], { cwd: temp, input, encoding: 'utf8' });
    const sb = spawnSync(lq, ['fmt', '--stdin-filepath', f], { cwd: temp, input, encoding: 'utf8' });
    if (a.out === b.out && a.code === b.code && sa.stdout === sb.stdout && sa.status === sb.status) same++;
    else { ok = false; console.log(`  ${f}: file exit ${a.code}/${b.code}, stdin exit ${sa.status}/${sb.status}\n   oracle ${JSON.stringify(a.out.slice(0, 300))}\n   lq     ${JSON.stringify(b.out.slice(0, 300))}\n   stdin oracle ${JSON.stringify(sa.stdout.slice(0, 300))}\n   stdin lq     ${JSON.stringify(sb.stdout.slice(0, 300))}`); }
  }
  console.log(`[fmt/config] per-file and stdin output: ${same}/${files.length} match`);
  // --write on two copies must leave the same trees.
  const copy = (name) => { const d = `${temp}-${name}`; fs.rmSync(d, { recursive: true, force: true }); fs.cpSync(temp, d, { recursive: true }); return d; };
  const da = copy('oracle'), db = copy('lq');
  const wa = run('node', [PRETTIER, '--write', '--cache-location', CACHE, '--plugin', PLUGIN, '**/*.liquid'], da);
  const wb = run(lq, ['fmt', '**/*.liquid', '--write'], db);
  const tree = (d) => { const out = {}; const w = (x) => { for (const e of fs.readdirSync(x, { withFileTypes: true })) { const p = path.join(x, e.name); if (e.isDirectory()) w(p); else out[path.relative(d, p)] = fs.readFileSync(p, 'utf8'); } }; w(d); return out; };
  const ta = tree(da), tb = tree(db);
  const differing = Object.keys({ ...ta, ...tb }).filter((k) => ta[k] !== tb[k]);
  console.log(`[fmt/config] --write: exit ${wa.status ?? wa.code}/${wb.status ?? wb.code}, differing files: ${JSON.stringify(differing)}`);
  ok = ok && differing.length === 0;
}
process.exit(ok ? 0 : 1);
