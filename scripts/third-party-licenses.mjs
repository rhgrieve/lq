#!/usr/bin/env node
// Prints the license texts of everything compiled into the `lq` binary, plus the upstream
// projects it was ported from. Every crate must have a license file, or this fails.
//   scripts/third-party-licenses.mjs [upstream dir] > THIRD_PARTY_LICENSES.md
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const cargo = (...args) => execFileSync('cargo', args, { cwd: ROOT, encoding: 'utf8', maxBuffer: 1 << 28 });
const meta = JSON.parse(cargo('metadata', '--format-version', '1', '--locked'));
const key = (p) => `${p.name} v${p.version}`;
const byKey = new Map(meta.packages.map((p) => [key(p), p]));
const names = new Set(
  cargo('tree', '-p', 'lq-cli', '-e', 'normal', '--target', 'all', '--prefix', 'none', '--format', '{p}', '--locked')
    .split('\n')
    .map((l) => l.match(/^\S+ v\S+/)?.[0])
    .filter(Boolean),
);

const LICENSE_FILE = /^(licen[cs]e|copying|notice|unlicense)/i;
const texts = new Map();
const table = [];
const missing = [];
for (const name of [...names].sort()) {
  const p = byKey.get(name);
  if (!p || p.source === null) continue;
  const dir = path.dirname(p.manifest_path);
  let files = fs.readdirSync(dir).filter((f) => LICENSE_FILE.test(f) && fs.statSync(path.join(dir, f)).isFile());
  // Crates from a workspace checkout, such as Biome's, keep their license at the root.
  for (let up = path.dirname(dir), i = 0; !files.length && i < 3; up = path.dirname(up), i++) {
    const found = fs.readdirSync(up).filter((f) => LICENSE_FILE.test(f) && fs.statSync(path.join(up, f)).isFile());
    if (found.length) files = found.map((f) => path.relative(dir, path.join(up, f)));
  }
  if (!files.length) missing.push(`${name} (${p.license})`);
  for (const f of files) {
    const text = fs.readFileSync(path.join(dir, f), 'utf8').trim();
    if (!texts.has(text)) texts.set(text, []);
    texts.get(text).push(p.name);
  }
  table.push(`| ${p.name} | ${p.version} | ${p.license ?? 'see license file'} |`);
}
if (missing.length) {
  console.error(`No license file found for:\n${missing.join('\n')}`);
  process.exit(1);
}

const out = [
  '# Third-party licenses',
  '',
  '`lq` is a port of MIT-licensed code from Shopify/theme-tools and of the printing algorithm of Prettier. Its binary',
  'embeds a docs snapshot from Shopify/theme-liquid-docs and compiles in the Rust crates listed below.',
  '',
];
const upstream = process.argv[2];
if (upstream) {
  for (const [title, file] of [
    ['Shopify/theme-tools and Shopify/theme-liquid-docs (MIT)', 'theme-tools/LICENSE.md'],
    ['Prettier (MIT)', 'prettier/LICENSE'],
  ]) {
    const p = path.join(upstream, file);
    if (!fs.existsSync(p)) {
      console.error(`Missing ${p}`);
      process.exit(1);
    }
    const text = fs.readFileSync(p, 'utf8');
    // Prettier's file goes on to list what its npm bundle includes, none of which is in lq.
    const own = file.startsWith('prettier') ? text.split(/\n#+ *Licenses of bundled dependencies/i)[0] : text;
    out.push(`## ${title}`, '', '```text', own.trim(), '```', '');
  }
}
out.push('## Rust crates', '', '| crate | version | license |', '|---|---|---|', ...table, '', '## Crate license texts', '');
for (const [text, crates] of texts) {
  out.push(`### ${[...new Set(crates)].join(', ')}`, '', '```text', text, '```', '');
}
process.stdout.write(out.join('\n'));
