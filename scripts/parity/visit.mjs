#!/usr/bin/env node
// Compares lq's visiting order with upstream's `visitLiquid` (type and ancestor
// count, in order) on every liquid file of the themes and the AST test cases.
import fs from 'node:fs';
import path from 'node:path';
import { GEN, THEMES, liquidFiles, oracleRequire, runLq } from './lib.mjs';

const { toLiquidHtmlAST, nonTraversableProperties } = oracleRequire('@shopify/liquid-html-parser');
const isNode = (t) => !!t && typeof t === 'object' && 'type' in t;

function visitLiquid(root) {
  const out = [];
  const stack = [{ node: root, depth: 0 }];
  while (stack.length) {
    const { node, depth } = stack.pop();
    out.push(`${node.type}:${depth}`);
    for (const key in node) {
      if (!Object.prototype.hasOwnProperty.call(node, key) || nonTraversableProperties.has(key)) continue;
      const value = node[key];
      if (Array.isArray(value)) {
        for (let i = value.length - 1; i >= 0; i--) {
          if (isNode(value[i])) stack.push({ node: value[i], depth: depth + 1 });
        }
      } else if (isNode(value)) {
        stack.push({ node: value, depth: depth + 1 });
      }
    }
  }
  return out.join(' ');
}

const files = Object.values(THEMES).flatMap(liquidFiles);
const cases = path.join(GEN, 'ast-cases');
if (fs.existsSync(cases)) {
  for (const f of fs.readdirSync(cases)) if (f.endsWith('.liquid')) files.push(path.join(cases, f));
}
const list = path.join(GEN, 'visit-files.list');
fs.writeFileSync(list, files.join('\n'));
const got = runLq(['debug-visit', '--files-from', list]).split('\n');
let diffs = 0;
files.forEach((f, i) => {
  let want;
  try {
    want = visitLiquid(toLiquidHtmlAST(fs.readFileSync(f, 'utf8')));
  } catch (e) {
    want = `error:${e.name}`;
  }
  // The parser reports its class name; both sides only need to agree it failed.
  const normal = (s) => (s?.startsWith('error:') ? 'error' : s);
  if (normal(want) !== normal(got[i])) {
    diffs++;
    if (diffs <= 5) {
      const a = want.split(' '), b = (got[i] ?? '').split(' ');
      const at = a.findIndex((x, j) => x !== b[j]);
      console.log(`  ${f}\n    at ${at}: want ${a.slice(Math.max(0, at - 2), at + 3).join(' ')}\n              got ${b.slice(Math.max(0, at - 2), at + 3).join(' ')}`);
    }
  }
});
console.log(`[visit] files=${files.length} diffs=${diffs}`);
process.exit(diffs ? 1 : 0);
