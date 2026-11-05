// Shared helpers for the differential parity harnesses.
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
export const ORACLE = path.join(ROOT, 'reference/oracle');
export const GEN = path.join(ROOT, 'reference/gen');
export const oracleRequire = createRequire(path.join(ORACLE, 'package.json'));
// LQ_EXTRA_THEME adds a theme of your own, such as a private one, to every harness as `extra`.
export const THEMES = {
  ...(process.env.LQ_EXTRA_THEME ? { extra: path.resolve(process.env.LQ_EXTRA_THEME) } : {}),
  dawn: path.join(ROOT, 'reference/corpus/dawn'),
  horizon: path.join(ROOT, 'reference/corpus/horizon'),
};

export function lqBin() {
  if (process.env.LQ_BIN) return process.env.LQ_BIN;
  const profile = process.env.LQ_PROFILE ?? 'release';
  return path.join(ROOT, 'target', profile, 'lq');
}

export function runLq(args, input) {
  return execFileSync(lqBin(), args, {
    maxBuffer: 1 << 30,
    encoding: 'utf8',
    input,
    stdio: ['pipe', 'pipe', 'pipe'],
  });
}

export function liquidFiles(dir) {
  const out = [];
  const walk = (d) => {
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      if (e.name === 'node_modules' || e.name === '.git') continue;
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (e.name.endsWith('.liquid')) out.push(p);
    }
  };
  walk(dir);
  return out.sort();
}

// Deterministic PRNG, so a failure reproduces across runs.
export function rng(seed) {
  let s = seed >>> 0;
  return (n) => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s % n;
  };
}

export function hash(str) {
  let h = 2166136261;
  for (let i = 0; i < str.length; i++) h = Math.imul(h ^ str.charCodeAt(i), 16777619);
  return h >>> 0;
}

// Malformed inputs. These exercise error paths and tolerant recovery.
export function breakSource(src, seed) {
  const r = rng(seed);
  if (src.length < 4) return [];
  const del = r(src.length);
  const ins = r(src.length);
  const junk = ['{%', '%}', '{{', '}}', '<', '>', '"', "'", '</div>', '{% endif %}', '-->'][r(11)];
  return [
    src.slice(0, r(src.length)),
    src.slice(0, del) + src.slice(del + 1 + r(12)),
    src.slice(0, ins) + junk + src.slice(ins),
  ];
}

// Valid-ish but badly formatted inputs. A clean theme is a weak formatter test.
export function messSource(src) {
  return [
    src.replace(/^[ \t]+/gm, ''),
    src.replace(/>\s*\n\s*</g, '><'),
    src.replace(/\{%-?\s+/g, '{%').replace(/\s+-?%\}/g, '%}').replace(/\{\{-?\s+/g, '{{').replace(/\s+-?\}\}/g, '}}'),
    src.replace(/\n\s*\n/g, '\n').replace(/([^\n])\n[ \t]*(\S)/g, (m, a, b) => (a === '>' || b === '<' ? `${a} ${b}` : m)),
  ];
}

// First path at which two JSON values differ, or null when equal.
export function firstDiff(a, b, p = '$') {
  if (a === b) return null;
  if (typeof a !== typeof b || a === null || b === null || typeof a !== 'object') {
    return { path: p, oracle: a, lq: b };
  }
  if (Array.isArray(a) !== Array.isArray(b)) return { path: p, oracle: a, lq: b };
  if (Array.isArray(a)) {
    if (a.length !== b.length) return { path: `${p}.length`, oracle: a.length, lq: b.length };
    for (let i = 0; i < a.length; i++) {
      const d = firstDiff(a[i], b[i], `${p}[${i}]`);
      if (d) return d;
    }
    return null;
  }
  for (const k of new Set([...Object.keys(a), ...Object.keys(b)])) {
    const d = firstDiff(a[k], b[k], `${p}.${k}`);
    if (d) return d;
  }
  return null;
}

export function snippet(v) {
  const s = JSON.stringify(v);
  return s === undefined ? 'undefined' : s.length > 240 ? `${s.slice(0, 240)}...` : s;
}

export function writeCases(dir, cases) {
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  return cases.map((c, i) => {
    const p = path.join(dir, `${String(i).padStart(6, '0')}.liquid`);
    fs.writeFileSync(p, c);
    return p;
  });
}
