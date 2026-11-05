#!/usr/bin/env node
// JSON embed gate: real schema bodies and hand-written cases, with seeded mutations that exercise
// Babel's tolerance (comments, quotes, numbers, holes, identifiers), run as {% schema %} blocks and
// <script type="application/json"> through the oracle and lq. Must be byte-identical.
// Usage: fmt-json.mjs [--seeds N] [--per N] [--show N] [--width N]
import fs from 'node:fs';
import path from 'node:path';
import { GEN, THEMES, liquidFiles, oracleRequire, rng, runLq, writeCases } from './lib.mjs';

const prettier = oracleRequire('prettier');
const plugin = oracleRequire('@shopify/prettier-plugin-liquid');
const { toTolerantLiquidHtmlAST, walk } = oracleRequire('@shopify/liquid-html-parser');
const args = process.argv.slice(2);
const val = (f, d) => (args.includes(f) ? args[args.indexOf(f) + 1] : d);
const seeds = Number(val('--seeds', 5));
const per = Number(val('--per', 3));
const show = Number(val('--show', 8));
const width = Number(val('--width', 100));

const HAND = [
  '{"a":1}', '{a:1}', "{'a':'b'}", '{"a":"it\'s"}', "{'a':\"q\\\"x\"}", '{1:2,1.5:3,1e3:4,.5:5,0x10:6}', '[1,,2,]', '[,]', '[,,]',
  '/* c */ {"a":1}', '{"a":1} // t', '{ // x\n "a":1 }', '{"a":/*c*/1}', '<!-- h\n{"a":1}', '+1', '-1', '- 1', '+Infinity', '-NaN', 'undefined',
  '-undefined', '`abc`', '`a\\nb`', '`a${1}`', '{"a":`b`}', 'null', 'true', 'NaN', '1n', '{a}', '{[a]:1}', '(1)', '{"__proto__":1,__proto__:2}',
  '"a"\n"b"', '{"a":1}{"b":2}', '{if:1,null:2,true:3}', '{\\u0061:1}', '{é:1}', '"a\\xZZ"', "'\\1'", '"a\\\nb"', '{"a":" "}', '0777', '089', '089.5',
  '1_000', '{1_000:1}', '[]', '{}', '[[],{}]', '[ ]', '/* only */', '{"a": {{ b }}}', '{% if a %}{}{% endif %}', '1.0', '{1.0:1}', '1.50', '1E3', '1e+3', '1e03', '.5', '5.', '0XAB', '1e00',
  '{100:1,0:1,0.1:1,1e21:1,1000000000000000000000:1}', '[1,\n\n2]', '{"a":1,\n\n"b":2}', '[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50]',
  '[-1,+2,3.5,-4e3]', '[1,// c\n2]', '[1, /* c */ 2]', '[// c\n1,\n2]', '[{"a":1,"b":2},{"a":3,"b":4}]', '[[1,2],[3,4]]', '[{"a":1},{"b":2}]', '[[1],[2]]', '[{"a":1,"b":2},[1,2]]',
  '{"a":{"b":{"c":1}}}', '{\n"a":{"b":1}}', '{"a":\n{"b":1}}', '{"a":[\n1]}', '{"a":1,// t\n"b":2}', '{"a":1,/* t */"b":2}', '{/* d */}', '{// d\n}', '[/* d */]', '[// d\n]', '{"a":// c\n1}', '{"a":/* c */\n1}', '{"a"/* c */:1}', '{/* c */"a":1}',
  '{// prettier-ignore\n"a":   1,\n"b":   2}', '[/* prettier-ignore */ 1,   2]', '{"a": /* prettier-ignore */   [1,   2]}', '/* prettier-ignore */ {"a":   1}', '// prettier-ignore\n{"a":   1}',
  '/**\n * doc\n */\n{"a":1}', '/*\n not\n indentable\n*/\n{"a":1}', '{"a":1}\n/* tail */', '{"a":1}\n\n// tail', '// a\n// b\n{"a":1}', '// a\n\n// b\n\n{"a":1}', '/* a */ /* b */ {"a":1}', '{"a":1} /* a */ /* b */', '{"a":1} // a\n// b',
  '/** @type {x} */ {"a":1}', '{"a": /** @type {x} */\n 1}', '- /* c */ 1', '-// c\n1', '--> c\n1', '1\n--> c', '1 --> c', '#!x\n1', '{"a":1,}', '[1,]', '{,}', '{"a"}', '{"a":}', '{:1}', '[1 2]', '{"a":1 "b":2}', 'a', 'this', '1+2', 'x=>1', '/re/', '`a\nb`', '{"a":`b\nc`}', '`\\u{110000}`', '`\\01`', '`\\8`', '"\\u{110000}"', '"\\u12"', "'a\nb'", '"a', "'", '`', '/*', '{"a":"b"',
  '{get:1,set:2,async:3,static:4}', '{get a(){}}', '{a(){}}', '{async a(){}}', '{*a(){}}', '{...a}', '[...a]', 'new a', 'void 0', 'typeof a', '!1', '~1', '1,2', '(1,2)', '{a:1}.a', '[1][0]', '1 ', ' 1', '\n\n1\n\n', '  1', '﻿1', ' 1', '\u00851',
  '1a', '0b2', '0o8', '0xg', '1e', '1e+', '1_', '1__0', '0_1', '._1', '1._1', '0n', '1.5n', '.', '..', '0.', '00', '09.', '09e1', '07.5', '07e1',
  '{"\\u005f_proto__":1,"__proto__":2}', '{"a\\\nb":1}', "{'a\"b':1}", "{'a\\'b':1}", '{"a\'b":1}', '{\\u{61}:1}', '{a\\u0020:1}', '\\u0061', 'n\\u0075ll', '{n\\u0075ll:1}', 'N\\u0061N', '{"":1}', '{"a":""}',
  '{"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","c":"d"}',
  '["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","c"]',
  '{"日本語":"日本語日本語日本語日本語日本語日本語日本語日本語日本語日本語日本語日本語日本語日本語日本語日本語日本語日本語日本語日本語","c":"👍"}',
];
const TOKENS = ['/* c */', '// c\n', '\n', '\n\n', ' ', ',', ':', '{', '}', '[', ']', '"', "'", '`', '1', '.5', '-', '+', 'a', 'null', 'true', 'NaN', 'undefined', '<!-- x\n', '-->', '\\', '/** @type {x} */', '/* prettier-ignore */', '// prettier-ignore\n', '/*\n * a\n */', '{{ x }}', '0x1F', '1e3'];

function mutate(body, seed) {
  const r = rng(seed);
  // Prefer token boundaries, where comments and whitespace are legal.
  const boundary = () => {
    const spots = [];
    for (let i = 0; i < body.length && spots.length < 5000; i++) if (/[{}[\],:]/.test(body[i])) spots.push(i + 1);
    return spots.length ? spots[r(spots.length)] : r(body.length + 1);
  };
  switch (r(6)) {
    case 0: case 1: { const i = boundary(); return body.slice(0, i) + TOKENS[r(TOKENS.length)] + body.slice(i); }
    case 2: { const i = r(body.length + 1); return body.slice(0, i) + TOKENS[r(TOKENS.length)] + body.slice(i); }
    case 3: { const i = r(body.length + 1); return body.slice(0, i) + body.slice(i + 1 + r(6)); }
    case 4: return body.replace(/"/g, (m) => (r(3) === 0 ? "'" : m));
    default: return body.replace(/\s+/g, (m) => (r(3) === 0 ? '' : m));
  }
}

const real = new Set();
for (const dir of Object.values(THEMES)) {
  for (const f of liquidFiles(dir)) {
    try {
      walk(toTolerantLiquidHtmlAST(fs.readFileSync(f, 'utf8')), (n) => {
        if (n.type === 'RawMarkup' && n.kind === 'json' && n.value.trim() !== '' && n.value.length < 4000) real.add(n.value.trim());
      });
    } catch { /* unparsable files have no blocks to borrow */ }
  }
}
const bodies = [];
for (const b of HAND) { bodies.push(b); for (let s = 1; s <= seeds; s++) for (let k = 0; k < per; k++) bodies.push(mutate(b, s * 1000 + k * 31 + b.length)); }
for (const b of real) for (let s = 1; s <= seeds; s++) for (let k = 0; k < per; k++) bodies.push(mutate(b, s * 2000 + k * 37 + b.length));
for (const b of real) bodies.push(b);

const wrap = (b, i) => {
  if (b.includes('endschema') || b.includes('</script')) return null;
  return i % 3 === 2 ? `<div>\n<script type="application/json">${b}</script>\n</div>\n` : `{% schema %}\n${b}\n{% endschema %}\n`;
};
const cases = bodies.map(wrap).filter(Boolean);
const widths = [width, 40];
let bad = 0, total = 0, verbatim = 0, shown = 0;
for (const w of widths) {
  const opts = { parser: 'liquid-html', plugins: [plugin], printWidth: w };
  const want = await Promise.all(cases.map(async (src) => {
    try { return { out: await prettier.format(src, opts) }; } catch (e) { return { error: String(e.message).split('\n')[0] }; }
  }));
  const files = writeCases(path.join(GEN, 'fmt-json-cases'), cases);
  const list = path.join(GEN, 'fmt-json-cases.list');
  fs.writeFileSync(list, files.join('\n'));
  const got = runLq(['debug-fmt', '--files-from', list, '--print-width', String(w), '--embedded', 'auto']).trim().split('\n').map((l) => JSON.parse(l));
  cases.forEach((src, i) => {
    total++;
    const a = want[i], b = got[i];
    if (a.out !== undefined && a.out === src) verbatim++;
    if (a.out === b.out && a.error === b.error) return;
    bad++;
    if (shown++ < show) {
      console.log(`  case (w${w}): ${JSON.stringify(src)}`);
      console.log(`    oracle| ${JSON.stringify(a.out ?? a.error)}`);
      console.log(`    lq    | ${JSON.stringify(b.out ?? b.error)}`);
    }
  });
}
console.log(`[fmt/json] ${total - bad}/${total} match, ${bad} differ (${verbatim} unchanged by the oracle)`);
process.exit(bad ? 1 : 0);
