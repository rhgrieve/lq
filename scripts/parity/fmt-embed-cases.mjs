#!/usr/bin/env node
// Hand-written blocks where Babel/PostCSS and Biome might disagree about what parses. Run inside a
// <script>/<style>; reports accept/reject per side and whether the whole document matches.
// Usage: fmt-embed-cases.mjs [--all]
import fs from 'node:fs';
import path from 'node:path';
import { GEN, oracleRequire, runLq, writeCases } from './lib.mjs';

const prettier = oracleRequire('prettier');
const plugin = oracleRequire('@shopify/prettier-plugin-liquid');
const all = process.argv.includes('--all');
const JS = [
  '<!-- x\nvar a = 1;', 'return 1;', 'with (a) {}', 'var a = 010;', 'var a = "\\01";', 'delete a;', '{{ a }}', '{{ a | b }}', '{{ a | b: 1 }}', 'var x = {{ y | json }};', 'var x = "{{ y }}";', '{% if a %}1{% endif %}',
  'var a = {{ y }}.z;', 'await x;', 'import a from "b";', 'export default 1;', 'var let;', 'var yield;', 'var await;', 'a = <div />;', 'class A { #x; }', '@dec class A {}', 'a ?? b;', 'a |> b;', 'x = do { 1 };', '#!/bin/x\n1;',
  '"use strict"; var a = 010;', 'function f(a, a) {}', 'let a; let a;', 'if (a) function b() {}', 'a: 1;', 'arguments;', 'for (var i = 0 in a);', 'var a = "</script>";', 'var a: number = 1;', '// @flow\nvar a: number = 1;', 'var a = <b>;', 'async () => await 1;',
  'new.target;', 'super.a;', 'import.meta.url;', 'import("a");', 'yield 1;', 'var enum;', 'var static;', 'class A { constructor() {} constructor() {} }', 'break;', 'continue;', 'a = 1 +;', 'function (){}', 'export { a };', 'export { a as "b" };', '0b12;', '1_000;', '1__0;', '09.5;', '1n;',
  '`${a`;', '/a/gg;', '/(/;', 'a?.b;', 'a?.[0];', 'a ||= 1;', 'class A { static {} }', 'for await (a of b);', 'label: function f() {}', 'var \\u0061;', 'throw\n1;', 'return\n1;', ';', ';;', '// only\n', '/* only */', '{}', '{ a: 1 }', '{ "a": 1 }', '[1, 2]', '(1)', 'a\nb', 'var a = 1 /* c */;',
  '<div>{a}</div>;', '<a:b />;', '<></>;', 'a < b > c;', '<T>a;', 'html`<a>`;', 'css`a{}`;', 'var s = `\n  a\n    b\n`;', '/*\n a\n   b\n*/\nvar a;', 'function f() {\n  /*\n a\n   b\n*/\n  return `\n x\n`;\n}', 'if (a) {\n  /**\n   * a\n   */\n}', 'var a = "\\\nb";',
];
const CSS = [
  '{{ a }}', 'a { color: {{ b }}; }', 'a { {{ b }}: c; }', '{{ a }} { b: c; }', '{% if a %}b{c:d}{% endif %}', 'a { {% if b %}c: d;{% endif %} }', '@media {{ a }} {}', '@media (min-width: {{ a }}px) { b { c: d; } }', 'a{b:c', 'a{', '}', 'b', '// c', '// c\na{b:c}', 'a{b:c;;}', 'a{color:red;}}', '@import "x";', '$x: 1;', '@include a;', 'a { &:hover{} }', '<!-- a{} -->', 'a{b}', '{}', 'a{}', ';', '@', 'a,{b:c}', 'a{b:}', 'a{:c}', 'a{b:c!important}', '@charset "a";', '@font-face{a:b}',
  '--x: 1;', 'a{--x:{a:b}}', 'a{--x: {{ b }};}', 'a{b:url({{ c }})}', 'a{b:"{{ c }}"}', '.a-{{ b }}{c:d}', '#{{ a }} {b:c}', '.a {{ b }} {c:d}', 'a{b:c}/* d', '/* a */', '/*\n  a\n    b\n*/\na{b:c}', 'a{/*\n b\n*/c:d}', 'a { b: calc(1 + {{ c }}); }', '@media screen{a{b:c}', 'a{b:c}}', '@supports (a:b){c{d:e}}', 'a::before{content:"\\201C"}', 'a{b:1px+2px}', 'A{COLOR:RED}', 'a{b:#FFF}', 'a>b+c~d{e:f}', '[a=b]{c:d}', 'a{b:c;d}', '@layer a;', '@container (a){b{c:d}}', '@apply a;', 'a{@apply b;}', '@tailwind base;', '@use "a";', '@mixin a{b:c}', '%a{b:c}', 'a{b:$c}', 'a{b:@c}', '@c: 1;', '.a{.b;}',
];
const cases = [...JS.map((b) => ({ kind: 'javascript', b, parser: 'babel', src: `<script>\n${b}\n</script>\n` })), ...CSS.map((b) => ({ kind: 'css', b, parser: 'css', src: `<style>\n${b}\n</style>\n` }))];
const dir = path.join(GEN, 'fmt-embed-cases');
fs.rmSync(dir, { recursive: true, force: true });
fs.mkdirSync(dir, { recursive: true });
const list = cases.map((c, i) => { const f = path.join(dir, `${i}.txt`); fs.writeFileSync(f, c.b); return JSON.stringify({ file: f, kind: c.kind, width: 98 }); });
const lf = path.join(GEN, 'fmt-embed-cases.list');
fs.writeFileSync(lf, list.join('\n'));
const alone = runLq(['debug-embed', lf]).trim().split('\n').map((l) => JSON.parse(l));
const files = writeCases(path.join(GEN, 'fmt-embed-docs'), cases.map((c) => c.src));
const dl = path.join(GEN, 'fmt-embed-docs.list');
fs.writeFileSync(dl, files.join('\n'));
const docs = runLq(['debug-fmt', '--files-from', dl, '--print-width', '100', '--embedded', 'auto']).trim().split('\n').map((l) => JSON.parse(l));
const tally = {};
for (const [i, c] of cases.entries()) {
  let oracleOk = true, msg = '';
  try { await prettier.format(c.b, { parser: c.parser, singleQuote: true, printWidth: 98 }); } catch (e) { oracleOk = false; msg = String(e.message).split('\n')[0]; }
  let doc;
  try { doc = await prettier.format(c.src, { parser: 'liquid-html', plugins: [plugin], printWidth: 100 }); } catch (e) { doc = `<err ${e.message}>`; }
  const biomeOk = !alone[i].error;
  const same = doc === docs[i].out;
  const key = `${c.kind} prettier=${oracleOk ? 'ok' : 'rej'} biome=${biomeOk ? 'ok' : 'rej'} ${same ? 'same' : 'DIFF'}`;
  tally[key] = (tally[key] ?? 0) + 1;
  if (all || oracleOk !== biomeOk || !same) {
    console.log(`${key}: ${JSON.stringify(c.b)}${msg ? `  <- ${msg}` : ''}`);
    if (!same && oracleOk === biomeOk) console.log(`    oracle| ${JSON.stringify(doc)}\n    lq    | ${JSON.stringify(docs[i].out)}`);
  }
}
console.table(tally);
