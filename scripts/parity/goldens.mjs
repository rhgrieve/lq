// Replays tests/goldens/*.jsonl through `lq debug-check-golden <file>`.
//
//   node scripts/parity/goldens.mjs [--show N] [--default-mode] [CheckCode...]
//
// Upstream's specs describe upstream, so this replays in `--upstream-bugs` mode.
// --default-mode replays with the fixes on instead, to see which specs a fix contradicts:
// the exit code then only reflects crashes.
//
// Prints `<CheckCode> passed/total/unreplayable` per check, then totals. Exits
// nonzero when an implemented check has failures or the runner misbehaves.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { ROOT, lqBin } from './lib.mjs';

const GOLDENS = path.join(ROOT, 'tests/goldens');

function parseArgs(argv) {
  const opts = { show: 0, codes: [], defaultMode: false };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--default-mode') {
      opts.defaultMode = true;
      continue;
    }
    if (argv[i] !== '--show') {
      opts.codes.push(argv[i]);
      continue;
    }
    opts.show = Number(argv[++i]);
    if (!Number.isInteger(opts.show) || opts.show < 0) die('--show needs a non-negative integer');
  }
  return opts;
}

function die(message) {
  console.error(message);
  process.exit(2);
}

function goldenFiles(codes) {
  if (!fs.existsSync(GOLDENS)) die(`no goldens at ${GOLDENS}; run scripts/parity/record-goldens.sh`);
  const stems = fs
    .readdirSync(GOLDENS)
    .filter((f) => f.endsWith('.jsonl'))
    .map((f) => f.slice(0, -'.jsonl'.length))
    .sort();
  const unknown = codes.filter((code) => !stems.includes(code));
  if (unknown.length > 0) die(`no golden file for: ${unknown.join(', ')}`);
  return codes.length > 0 ? codes : stems;
}

// Returns the runner's report, or { crash } when it broke its contract.
function runGolden(bin, stem, defaultMode) {
  const mode = defaultMode ? [] : ['--upstream-bugs'];
  const res = spawnSync(bin, ['debug-check-golden', path.join(GOLDENS, `${stem}.jsonl`), ...mode], {
    encoding: 'utf8',
    maxBuffer: 1 << 30,
  });
  if (res.error) return { crash: res.error.message };
  try {
    const report = JSON.parse(res.stdout);
    if (res.status !== 0 && !report.failed) return { crash: `exit ${res.status}: ${res.stderr}` };
    return report;
  } catch {
    return { crash: `exit ${res.status}, bad JSON on stdout: ${res.stderr || res.stdout}`.slice(0, 500) };
  }
}

const { show, codes, defaultMode } = parseArgs(process.argv.slice(2));
const stems = goldenFiles(codes);
const bin = lqBin();
if (!fs.existsSync(bin)) {
  die(`lq binary not found at ${bin}. Build it (cargo build --release) or set LQ_BIN.`);
}

const totals = { passed: 0, total: 0, unreplayable: 0, failed: 0 };
const notImplemented = [];
const crashed = [];
for (const stem of stems) {
  const report = runGolden(bin, stem, defaultMode);
  if (report.crash) {
    crashed.push(stem);
    console.log(`${stem} CRASH ${report.crash}`);
    continue;
  }
  const name = report.check ?? stem;
  const suffix = report.notImplemented ? ' NOT-IMPLEMENTED' : '';
  console.log(`${name} ${report.passed}/${report.total}/${report.unreplayable}${suffix}`);
  totals.passed += report.passed;
  totals.total += report.total;
  totals.unreplayable += report.unreplayable;
  totals.failed += report.failed;
  if (report.notImplemented) notImplemented.push(name);
  for (const failure of (report.failures ?? []).slice(0, show)) {
    console.log(`  FAIL ${failure.test}\n${failure.diff}`);
  }
}

console.log(
  `TOTAL ${totals.passed}/${totals.total}/${totals.unreplayable} ` +
    `failed=${totals.failed} crashed=${crashed.length} notImplemented=${notImplemented.length}`,
);
if (notImplemented.length > 0) console.log(`Not implemented: ${notImplemented.join(' ')}`);
process.exit((totals.failed > 0 && !defaultMode) || crashed.length > 0 ? 1 : 0);
