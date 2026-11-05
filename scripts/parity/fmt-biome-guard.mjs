#!/usr/bin/env node
// A panic in Biome must not end the run. Injects one in each Biome step of the binary under
// test, which proves the release profile unwinds: the output must equal `--embedded off`,
// the exit code 0, and stderr must carry a warning.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { GEN, lqBin } from './lib.mjs';

const dir = path.join(GEN, 'fmt-biome-guard');
fs.mkdirSync(dir, { recursive: true });
const file = path.join(dir, 'case.liquid');
const list = path.join(dir, 'list');
fs.writeFileSync(file, '<style>\na { color:   red }\n</style>\n<script>\nlet   a=1\n</script>\n');
fs.writeFileSync(list, `${file}\n`);
const run = (...flags) => spawnSync(lqBin(), ['debug-fmt', '--files-from', list, ...flags], { encoding: 'utf8' });

const off = run('--embedded', 'off').stdout;
const on = run().stdout;
let failed = on === off ? 1 : 0;
if (on === off) console.log('[fmt/biome-guard] FAIL: Biome left the input alone, so this proves nothing');
for (const step of ['parse', 'format']) {
  const r = run('--inject-biome-panic', step);
  const good = r.status === 0 && r.stdout === off && /Biome panicked/.test(r.stderr) && !/panicked at/.test(r.stderr);
  console.log(`[fmt/biome-guard] panic in ${step}: exit ${r.status}, verbatim ${r.stdout === off}, warnings ${r.stderr.split('\n').filter(Boolean).length}: ${good ? 'ok' : 'FAIL'}`);
  if (!good) {
    failed++;
    console.log(r.stderr.slice(0, 400));
  }
}
process.exit(failed ? 1 : 0);
