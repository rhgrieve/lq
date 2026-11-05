#!/usr/bin/env node
// Compares `lq check --output json` with upstream's themeCheckRun.
// Usage: lint.mjs [extra|dawn|horizon|<path>]... [--show N] [--default-mode]
//
// Runs `lq check --upstream-bugs`, which must match upstream exactly. With --default-mode it
// runs the default fixes instead, and groups each difference under the divergence that
// causes it: the first whose fix alone (`--only-divergence`) produces that difference. A
// difference no fix explains is a failure.
import path from 'node:path';
import { THEMES, oracleRequire, runLq, snippet } from './lib.mjs';

const { themeCheckRun } = oracleRequire('@shopify/theme-check-node');
const args = process.argv.slice(2);
const show = args.includes('--show') ? Number(args[args.indexOf('--show') + 1]) : 40;
const defaultMode = args.includes('--default-mode');
const names = args.filter((a, i) => !a.startsWith('--') && args[i - 1] !== '--show');
const divergenceIds = runLq(['debug-divergences']).trim().split('\n').map((l) => l.split('\t')[0]);
const targets = names.length ? names : Object.keys(THEMES);
const SEVERITY = ['error', 'warning', 'info'];
const key = (o) => JSON.stringify([o.file, o.check, o.severity, o.startRow, o.startColumn, o.endRow, o.endColumn, o.message]);

function lqIssues(root, flags) {
  let out = '';
  try {
    out = runLq(['check', '--path', root, '--output', 'json', ...flags]);
  } catch (e) {
    out = e.stdout ?? '';
    if (!out) throw e;
  }
  return JSON.parse(out).issues.map((o) => ({ ...o, file: path.relative(root, o.filePath) }));
}

const countOf = (l) => l.reduce((m, o) => m.set(key(o), (m.get(key(o)) ?? 0) + 1), new Map());
const diffOf = (w, g) => ({
  missing: [...w].filter(([k, n]) => (g.get(k) ?? 0) < n).map(([k]) => k),
  extra: [...g].filter(([k, n]) => (w.get(k) ?? 0) < n).map(([k]) => k),
});

// `--default-mode`: every difference, grouped by the divergence that causes it.
function reportDefaultMode(name, root, want, w) {
  const d = diffOf(w, countOf(lqIssues(root, [])));
  const removed = new Set(d.missing), added = new Set(d.extra);
  console.log(`[lint/${name}/default] oracle=${want.length} missing=${removed.size} extra=${added.size}`);
  let explained = 0;
  for (const id of divergenceIds) {
    const alone = diffOf(w, countOf(lqIssues(root, ['--only-divergence', id])));
    const gone = alone.missing.filter((k) => removed.delete(k));
    const more = alone.extra.filter((k) => added.delete(k));
    if (!gone.length && !more.length) continue;
    explained += gone.length + more.length;
    const perCheck = {};
    for (const k of [...gone, ...more]) perCheck[JSON.parse(k)[1]] = (perCheck[JSON.parse(k)[1]] ?? 0) + 1;
    console.log(`  ${id}: ${gone.length} removed, ${more.length} added ${snippet(perCheck)}`);
    gone.slice(0, show).forEach((k) => console.log(`    - ${k}`));
    more.slice(0, show).forEach((k) => console.log(`    + ${k}`));
  }
  const unexplained = removed.size + added.size;
  console.log(`  explained=${explained} unexplained=${unexplained}`);
  [...removed].slice(0, show).forEach((k) => console.log(`  UNEXPLAINED - ${k}`));
  [...added].slice(0, show).forEach((k) => console.log(`  UNEXPLAINED + ${k}`));
  return unexplained === 0;
}

const implemented = new Set(
  runLq(['debug-checks'])
    .split('\n')
    .filter(Boolean)
    .map((l) => JSON.parse(l))
    .filter((c) => c.implemented)
    .map((c) => c.code),
);
let failed = false;
let pending = false;
for (const name of targets) {
  const root = THEMES[name] ?? path.resolve(name);
  const want = (await themeCheckRun(root, undefined, () => {})).offenses.map((o) => ({
    file: path.relative(root, o.uri.replace(/^file:\/\//, '')),
    check: o.check,
    severity: SEVERITY[o.severity],
    // The CLI's JSON is one-based: `Math.max(x + 1, 1)` in its ThemeCheckResult.
    startRow: Math.max(o.start.line + 1, 1),
    startColumn: Math.max(o.start.character + 1, 1),
    endRow: Math.max(o.end.line + 1, 1),
    endColumn: Math.max(o.end.character + 1, 1),
    message: o.message,
  }));
  const w = countOf(want);
  if (defaultMode) {
    if (!reportDefaultMode(name, root, want, w)) failed = true;
    continue;
  }
  const got = lqIssues(root, ['--upstream-bugs']);
  const g = countOf(got);
  const { missing, extra } = diffOf(w, g);
  console.log(`[lint/${name}] oracle=${want.length} lq=${got.length} missing=${missing.length} extra=${extra.length}`);
  const perCheck = {};
  for (const k of [...missing, ...extra]) perCheck[JSON.parse(k)[1]] = (perCheck[JSON.parse(k)[1]] ?? 0) + 1;
  if (Object.keys(perCheck).length) console.log('  by check:', snippet(perCheck));
  const isImplemented = (k) => implemented.has(JSON.parse(k)[1]);
  const inImplemented = [...missing, ...extra].filter(isImplemented);
  const pendingCount = missing.length + extra.length - inImplemented.length;
  console.log(`  implemented-checks diffs=${inImplemented.length}, not-yet-implemented diffs=${pendingCount}`);
  // File order and order within a file, for the implemented checks only.
  const ofImplemented = (l) => l.filter((o) => implemented.has(o.check));
  const firstFiles = (l) => [...new Set(ofImplemented(l).map((o) => o.file))];
  const wantFiles = firstFiles(want), gotFiles = firstFiles(got);
  const sameFileOrder = JSON.stringify(wantFiles) === JSON.stringify(gotFiles);
  const perFile = (l) => {
    const m = new Map();
    for (const o of ofImplemented(l).slice().sort((a, b) => SEVERITY.indexOf(a.severity) - SEVERITY.indexOf(b.severity))) {
      if (!m.has(o.file)) m.set(o.file, []);
      m.get(o.file).push(key(o));
    }
    return m;
  };
  const wf = perFile(want), gf = perFile(got);
  let inFileDiffs = 0;
  for (const [f, l] of wf) if (JSON.stringify(l) !== JSON.stringify(gf.get(f))) inFileDiffs++;
  console.log(`  order (implemented checks): file order ${sameFileOrder ? 'same' : 'differs'}, files with different in-file order ${inFileDiffs}/${wf.size}`);
  missing.filter(isImplemented).slice(0, show).forEach((k) => console.log(`  - missing ${k}`));
  extra.filter(isImplemented).slice(0, show).forEach((k) => console.log(`  + extra   ${k}`));
  if (inImplemented.length) failed = true;
  if (missing.length || extra.length) pending = true;
}
if (pending && !failed) console.log(`[lint] only not-yet-implemented checks differ (${implemented.size} implemented)`);
// Exit 1 on a diff in an implemented check, 3 when only unported checks differ.
process.exit(failed ? 1 : pending ? 3 : 0);
