#!/usr/bin/env node
// Differential coverage for LiquidSyntaxError: upstream's real check, through check() with
// only this rule, against lq's port. Most of upstream's spec doesn't go through check(), so
// the goldens can't cover it alone.
//
//   liquid-syntax.mjs [--seeds N] [--show N] [--only <suite>]
//
// Each suite is written to reference/gen/liquid-syntax/<suite>.jsonl as golden records and
// replayed by `lq debug-check-golden`, which compares messages, indexes, lines, characters
// and order. Exits 1 on any difference.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { GEN, ROOT, THEMES, breakSource, hash, liquidFiles, lqBin, oracleRequire } from './lib.mjs';

const { check, allChecks, toSourceCode } = oracleRequire('@shopify/theme-check-common');
const YAML = oracleRequire('yaml');

const args = process.argv.slice(2);
const opt = (name, fallback) => (args.includes(name) ? args[args.indexOf(name) + 1] : fallback);
const SEEDS = Number(opt('--seeds', 3));
const SHOW = Number(opt('--show', 20));
const ONLY = opt('--only', null);
const OUT = path.join(GEN, 'liquid-syntax');
const SPEC_DIR = path.join(
  ROOT,
  'reference/theme-tools/packages/theme-check-common/src/checks/liquid-syntax-error',
);

const definition = allChecks.find((c) => c.meta.code === 'LiquidSyntaxError');
if (!definition) throw new Error('the oracle has no LiquidSyntaxError');

const noFs = {
  stat: async (uri) => { throw new Error(`no such file ${uri}`); },
  readFile: async (uri) => { throw new Error(`no such file ${uri}`); },
  readDirectory: async () => [],
};

const NO_DOCSET = null;
const TAG_NAMES = ['assign', 'if', 'endif', 'block', 'custom_tag', 'liquid', 'echo', 'hello'];
const TAGS_DOCSET = {
  filters: [],
  objects: [],
  liquidDrops: [],
  tags: TAG_NAMES.map((name) => ({ name })),
  systemTranslations: {},
};

function liveDocset(table) {
  if (!table) return undefined;
  return {
    filters: async () => table.filters,
    objects: async () => table.objects,
    liquidDrops: async () => table.liquidDrops,
    tags: async () => table.tags,
    systemTranslations: async () => table.systemTranslations,
  };
}

async function record(test, source, file, docset) {
  const uri = `file:///${file}`;
  let rejected;
  const offenses = await check(
    [toSourceCode(uri, source)],
    {
      settings: {},
      checks: [definition],
      rootUri: 'file:///',
      context: 'theme',
      onError: (e) => { rejected = e; },
    },
    { fs: noFs, themeDocset: liveDocset(docset) },
  );
  const rec = {
    spec: 'scripts/parity/liquid-syntax.mjs',
    test,
    theme: { [file]: source },
    checks: ['LiquidSyntaxError'],
    settings: {},
    mode: 'theme',
    deps: { themeDocset: docset },
    offenses: offenses.map((o) => ({ ...o, fix: o.fix ?? null, suggest: o.suggest ?? null })),
    fixed: {},
  };
  if (rejected) rec.error = { message: rejected.message };
  return rec;
}

// Slicing can split a surrogate pair. Lone surrogates can't reach lq, whose sources are UTF-8.
const wellFormed = (s) => s.toWellFormed();

class Suite {
  constructor(name) {
    this.name = name;
    this.seen = new Set();
    this.lines = [];
    this.offenses = 0;
    this.rejected = 0;
    this.errored = 0;
  }
  async add(test, raw, docsets = [NO_DOCSET], file = 'snippets/test.liquid') {
    const source = wellFormed(raw);
    for (const docset of docsets) {
      const key = `${docset ? 1 : 0}\0${file}\0${source}`;
      if (this.seen.has(key)) continue;
      this.seen.add(key);
      const rec = await record(test, source, file, docset);
      this.offenses += rec.offenses.length;
      if (rec.error) this.rejected++;
      this.lines.push(JSON.stringify(rec));
    }
  }
}

// The plain strings just before a template: the it.each table that feeds its holes.
function nearbyLiterals(ts, at) {
  const found = ts.slice(Math.max(0, at - 2500), at).match(/'(?:[^'\\\n]|\\.)*'|"(?:[^"\\\n]|\\.)*"/g) ?? [];
  const values = [];
  for (const f of found.slice(-60)) {
    try { values.push(new Function(`return ${f}`)()); } catch {}
  }
  return [...new Set(values)];
}

// Every value in the first hole, and for the rest the same values one at a time.
function fillHoles(template, values) {
  const out = [];
  for (const a of values) {
    for (const b of values.slice(0, 12)) {
      let n = 0;
      const text = template.replace(/\$\{\s*[^}]*\}/g, () => (n++ === 0 ? a : b).replace(/[`\\$]/g, '\\$&'));
      try { out.push(new Function(`return ${text}`)()); } catch {}
      if (n < 2) break;
    }
  }
  return out;
}

// String and template literals of a TypeScript file. `${...}` holes are filled with the
// spec's own constants where it has them, and the literal is dropped otherwise.
function literalsOf(ts, constants = {}) {
  const out = [];
  let dropped = 0;
  let i = 0;
  while (i < ts.length) {
    const c = ts[i];
    if (c === '/' && ts[i + 1] === '/') {
      while (i < ts.length && ts[i] !== '\n') i++;
    } else if (c === '/' && ts[i + 1] === '*') {
      i = ts.indexOf('*/', i + 2) + 2;
    } else if (c === "'" || c === '"' || c === '`') {
      let j = i + 1;
      let holes = false;
      let depth = 0;
      while (j < ts.length && (depth > 0 || ts[j] !== c)) {
        if (ts[j] === '\\') j++;
        else if (c === '`' && ts[j] === '$' && ts[j + 1] === '{') { holes = true; depth++; j++; }
        else if (depth > 0 && ts[j] === '}') depth--;
        j++;
      }
      let text = ts.slice(i, j + 1);
      if (holes) text = text.replace(/\$\{\s*(\w+)\s*\}/g, (m, n) => (n in constants ? constants[n] : m));
      if (text.includes('${')) {
        const filled = fillHoles(text, nearbyLiterals(ts, i));
        if (filled.length === 0) dropped++;
        out.push(...filled);
      } else {
        try { out.push(new Function(`return ${text}`)()); } catch { dropped++; }
      }
      i = j + 1;
    } else {
      i++;
    }
  }
  return { literals: out, dropped };
}

const isLiquid = (s) => typeof s === 'string' && (s.includes('{%') || s.includes('{{') || s.includes('%}') || s.includes('}}'));

const FRAGMENTS = [
  '', ' ', '\n', 'x', 'x y', 'a.b', "a['b']", 'a[0]', '[0]', '[a]', '[0].b', 'a[[0]]', "'s'", '"s"', "'s", 's\'', '"s', "'a\" b", 'x "', "' x", "x '",
  '1', '-1', '1.5', '-', '.', '..', '(1..3)', '(1..', '(a..b)', '(1...3)', 'true', 'nil', 'empty', 'blank', 'toString', '__proto__',
  'x?', 'x??', '?', '?x', '!', 'x!', '!=', '@', '@x', 'x@', '#', '#x', '$', '%', '&', '*', '/', '\\', '^', '~', '`', ';', '{', '}',
  ',', 'x,', 'x,,', ',x', 'x, y', 'x, y,', 'x y,', ':', 'x:', 'x: 1', 'x: 1,', 'x: [0]', 'x: [1, 2]', 'x.y: 1', 'block.name: 1', "block.name: 's'", 'block.settings.a: 1', 'block.settings.a.b: 1', 'block.settings: 1', 'block..a: 1',
  '|', 'x |', 'x | f', 'x | f:', 'x | f: ', 'x | f: 1', 'x | f: 1,', 'x | f: 1, | g', 'x | f: 1, 2, | g:', 'x | f: , | g', 'x | f:,', 'x | f, | g', 'x | f: [0]', 'x | f: k: [0]', 'x || f', '| f', 'x | f: 1 2', 'x | f: @', 'x foo | f:', 'x = y | f:', 'x = y | f: 1,',
  '=', 'x =', '= x', 'x = y', 'x y = z', 'x 1 = z', 'x y = z | f', 'x = [0]', 'x = y[0]', 'x = @', '@x = y', "'x = y", "' x = y", '"x = y', 'x == y', 'x = = y', 'x.y = z', 'x[0] = z', 'x = y z',
  'x == 1', 'x != 1', 'x > 1', 'x >= [0]', '[0] == x', 'x contains y', 'x and y', 'x or [0]', '[0] and y', 'x and', 'and x', 'x and @', '@ x', 'x @', "x == 'a", "' x", "'x", "' x y", '(x)', '(x and y)', 'x <> y', 'x === y',
  'x in y', 'x in y z', 'x in', 'in y', 'x in [0]', 'x in y limit: 1', 'x in y limit: [0]', 'x in y, limit: 1', 'x in y,', 'x in y,,', 'x in y reversed', 'x in y reversed,', 'x in (1..3)', 'x in @', '@x in y', "'x in y", 'x in y @', 'x in y limit:', 'x in y cols: 2,',
  'x by 1', 'x by', 'by 1', 'x by [0]', '[0] by 1', 'x by 1,', 'x by 1,,', 'x by 1, window: 2', 'x by 1, window: [0]', 'x by 1 @', '@x by 1', "'x by 1", 'x by y z',
  "'a': b", "'a': b, c", "a: b, c", "'a', 'b'", "'a', 'b',", "'a', 'b',,", "x: 'a', 'b'", '[0]: a', 'a: [0], b', "'a', [0]", '[0], a',
  "'p', x: 1", "'p', x: [0]", "'p' for y", "'p' with y", "'p' for [0]", "'p' with [0] as z", "'p' as z", "'p' as", "'p', 'q'", "'p' x", "'p' @", "@ 'p'", 'p', '[0]', "'p', x.y: 1", "'p', x: 1,", "'p',", "'p' for y as z, k: v", '42', '(1..5) for products as item', "'p", "\"p' x",
  "'block', type: 's', id: 'i'", "'block', type: 's'", "'block', id: 'i'", "'block', type: x, id: 'i'", "'block', type: '', id: 'i'", "'block', type: 's', id: ''", "'block', type: 's', id: 'i', block: 1", "'block', type: 's', id: 'i', schema: 1", "'block', type: 's', type: 't', id: 'i'", "'block', type: 's', id: 'i', closest.product: x", "'block', type: 's', id: 'i', closest.product: 's'", "'block', type: 's', id: 'i', closest.product: x, closest.page: y", "'block', type: 's', id: 'i', closest.nope: x", "'block', type: 's', id: 'i', closest.metaobject.t: x", "'block', type: 's', id: 'i', k: [0]", "'blocks'", "'blocks', closest.product: x", "'blocks', closest.product: 's'", "'blocks', k: 1", "'blocks', closest.nope: x", "'blocks', closest.page: x, closest.blog: y", "'blocks', closest.metaobject.a.b: x", "'other'", "''", 'blocks', "'blocks' @", "'blocks', closest.product: [0]",
  "'shopify://apps/a/blocks/b/12345678-1234-1234-1234-123456789012'", "'shopify://apps/a/blocks/b/12345678-1234-1234-1234-123456789012', x: 1", "'shopify://apps/a/blocks/b/12345678-1234-1234-1234-123456789012', block.name: 's'", "'shopify://apps/a/blocks/b/12345678-1234-1234-1234-123456789012', block.name: x", "'shopify://apps/a'", "'bad name'", "'_ok'", "'ok', a: [1, [2]]", "'ok', a: [0", "'ok' @", "'ok', @", "'ok', a: b.c, d: e[0]",
  "'a/b'", "''", "'a'", "'a' b", "@ 'a'", 'a',
  'scss', "'scss'", '"scss"', "'scss' x", "'sass'", 'scss?', "'scss' @",
  // Whitespace the tokenizer skips but isWhitespace doesn't.
  ' ', 'x y', 'x ', ' x', '\f', 'x\fy', '\v', 'x\v', '﻿', ' x', '\r\n', 'x\r\ny', '\t', 'x\ty', '　',
  // Non-ASCII, in and out of strings.
  'é', 'x é', "'é'", "'é", '😀', 'x 😀', "'😀'", "'😀", '😀x', 'x | f: 😀', 'x = 😀', '😀 = x', 'x́',
  // Continuation lines for inline comments.
  'a\n# b', 'a\nb', '\nb', '\n# b', '\nb\nc', 'a\n\n# b', 'a\n  # b', "a\n'# b'", "a\n'#", "'a\n#b'", 'a\n#', '\n\n', 'é\nb', 'a\n😀', "a\n# b\nc", 'a\r\n# b',
];

const TAG_SHAPES = [
  ['output', (m) => `{{ ${m} }}`], ['output-tight', (m) => `{{${m}}}`], ['output-trim', (m) => `{{- ${m} -}}`],
  ['echo', (m) => `{% echo ${m} %}`], ['assign', (m) => `{% assign ${m} %}`], ['assign-rhs', (m) => `{% assign v = ${m} %}`],
  ['if', (m) => `{% if ${m} %}a{% endif %}`], ['unless', (m) => `{% unless ${m} %}a{% endunless %}`],
  ['elsif', (m) => `{% if a %}b{% elsif ${m} %}c{% endif %}`], ['case', (m) => `{% case ${m} %}{% when 1 %}a{% endcase %}`],
  ['when', (m) => `{% case a %}{% when ${m} %}b{% else %}c{% endcase %}`],
  ['for', (m) => `{% for ${m} %}a{% endfor %}`], ['tablerow', (m) => `{% tablerow ${m} %}a{% endtablerow %}`],
  ['paginate', (m) => `{% paginate ${m} %}a{% endpaginate %}`], ['cycle', (m) => `{% cycle ${m} %}`],
  ['capture', (m) => `{% capture ${m} %}a{% endcapture %}`], ['increment', (m) => `{% increment ${m} %}`], ['decrement', (m) => `{% decrement ${m} %}`],
  ['include', (m) => `{% include ${m} %}`], ['render', (m) => `{% render ${m} %}`], ['section', (m) => `{% section ${m} %}`], ['sections', (m) => `{% sections ${m} %}`],
  ['layout', (m) => `{% layout ${m} %}`], ['form', (m) => `{% form ${m} %}a{% endform %}`], ['block', (m) => `{% block ${m} %}{% endblock %}`], ['block-body', (m) => `{% block ${m} %} body {{ a }}{% endblock %}`],
  ['content_for', (m) => `{% content_for ${m} %}`], ['partial', (m) => `{% partial ${m} %}{% endpartial %}`], ['ifchanged', (m) => `{% ifchanged ${m} %}a{% endifchanged %}`],
  ['break', (m) => `{% break ${m} %}`], ['continue', (m) => `{% continue ${m} %}`], ['inline-comment', (m) => `{% # ${m} %}`], ['inline-comment-tight', (m) => `{%#${m}%}`],
  ['comment-markup', (m) => `{% comment ${m} %}a{% endcomment %}`], ['raw', (m) => `{% raw ${m} %}a{% endraw %}`], ['doc', (m) => `{% doc ${m} %}a{% enddoc %}`],
  ['javascript', (m) => `{% javascript ${m} %}a{% endjavascript %}`], ['schema', (m) => `{% schema ${m} %}{}{% endschema %}`], ['style', (m) => `{% style ${m} %}a{% endstyle %}`], ['stylesheet', (m) => `{% stylesheet ${m} %}a{% endstylesheet %}`],
  ['unknown', (m) => `{% foobar ${m} %}`], ['else', (m) => `{% if a %}{% else ${m} %}b{% endif %}`],
  ['endraw-markup', (m) => `{% raw %}a{% endraw ${m} %}`], ['enddoc-markup', (m) => `{% doc %}a{% enddoc ${m} %}`], ['endcomment-markup', (m) => `{% comment %}a{% endcomment ${m} %}`], ['endjs-markup', (m) => `{% javascript %}a{% endjavascript ${m} %}`],
  ['comment-body', (m) => `{% comment %}${m}{% endcomment %}`], ['doc-body', (m) => `{% doc %}${m}{% enddoc %}`], ['raw-body', (m) => `{% raw %}${m}{% endraw %}`],
  ['attr', (m) => `<a href="{{ ${m} }}" {% if ${m} %}b{% endif %}>c</a>`], ['script', (m) => `<script>{{ ${m} }}</script>`],
  // The same tags as {% liquid %} statements.
  ['l-echo', (m) => `{% liquid\n  echo ${m}\n%}`], ['l-assign', (m) => `{% liquid\n  assign ${m}\n%}`], ['l-if', (m) => `{% liquid\n  if ${m}\n    echo a\n  endif\n%}`],
  ['l-for', (m) => `{% liquid\n  for ${m}\n    echo a\n  endfor\n%}`], ['l-render', (m) => `{% liquid\n  render ${m}\n%}`], ['l-comment', (m) => `{% liquid\n  # ${m}\n%}`],
  ['l-block', (m) => `{% liquid\n  block ${m}\n  endblock\n%}`], ['l-unknown', (m) => `{% liquid\n  foobar ${m}\n%}`], ['l-case', (m) => `{% liquid\n  case a\n    when ${m}\n      echo b\n  endcase\n%}`],
  ['l-nested', (m) => `{% liquid\n  if a\n    nope ${m}\n  endif\n  nope2\n  liquid nope3\n%}`],
];

const PROTO_NAMES = ['constructor', 'toString', 'valueOf', 'hasOwnProperty', 'isPrototypeOf', 'propertyIsEnumerable', 'toLocaleString', '__proto__', '__defineGetter__', '__defineSetter__', '__lookupGetter__', '__lookupSetter__'];
const UNCLOSED_NAMES = ['comment', 'doc', 'raw', 'javascript', 'schema', 'style', 'stylesheet', 'block', 'partial', 'if', 'for', 'form', 'capture', 'case', 'paginate', 'tablerow', 'unless', 'ifchanged'];

function handWritten() {
  const out = [];
  for (const name of PROTO_NAMES) {
    out.push(`{% ${name} %}`, `{% liquid\n  ${name}\n%}`, `{% liquid\n  ${name} a\n  nope\n%}`, `{{ a }}{% liquid\n  nope\n  ${name}\n%}{{ [0] }}`, `{{ ${name} }}`);
  }
  for (const n of UNCLOSED_NAMES) {
    const end = `{% end${n} %}`;
    const open = `{% ${n} %}`;
    out.push(
      open, end, `a${open}`, `a\nb${end}`, `${open}😀`, `😀${open}`, `😀\n${end}`, `${end}😀`, `x\n😀${open}\n😀`, `${open}\n`, `\n${open}`,
      `${open}${open}${end}`, `${open}${open}${end}${end}`, `${open}${end}${end}`, `${end}${open}`, `${open}${end} ${end} ${open}`,
      `{% ${n} x %}`, `{% ${n} @ %}`, `{%- ${n} -%}`, `{%-${n}-%}${end}`, `{% end${n} x %}`, `${open}{% end${n} x %}`, `${open}{% end${n} @ %}`, `${open}{% end${n} x %}${open}`, `${open}{% end${n} x %}${open}${end}`, `${open}{% end${n} x %}${end}`,
      `{% ${n}x %}`, `{% ${n}x %}${end}`, `${open}{% ${n}x %}`, `{{ ${n} }}${end}`, `${n} text ${end}`,
      `{% liquid\n  ${n}\n%}`, `{% liquid\n  ${n}\n  echo a\n%}`, `{% liquid\n  end${n}\n%}`, `{% liquid\n  echo a\n  ${n} 'x'\n%}`, `{% liquid\n  ${n} 'x'\n  end${n}\n%}`, `{% liquid\n  echo a\n%}{% liquid\n  ${n}\n%}`, `😀{% liquid\n  😀\n  ${n}\n%}`, `{% liquid ${n}\n%}`, `{% liquid\n${n} %}`, `{% comment %}{% liquid\n  ${n}\n%}{% endcomment %}{% liquid\n  ${n}\n%}`,
      `{% comment %}${open}{% endcomment %}`, `{% comment %}${end}{% endcomment %}`, `{% raw %}${open}{% endraw %}`, `{% raw %}${end}{% endraw %}`, `{% doc %}${open}{% enddoc %}`, `{% doc %}${end}{% enddoc %}`,
      `<div>${open}</div>`, `<div>${end}</div>`, `${open}<div>${end}</div>`, `<a b="${open}">`, `<a b="${end}">`,
    );
  }
  out.push(
    '', ' ', '\n', '😀', '{%', '%}', '{{', '}}', '{% %}', '{%%}', '{{}}', '{{ }}', '{%- -%}', '{{- -}}', '{% # %}', '{%#%}', '{% if %}', '{% assign %}', '{% echo %}', '{% render %}', '{% for %}', '{% block %}{% endblock %}',
    '{% else %}', '{% elsif a %}', '{% when a %}', '{% liquid\n  else\n%}', '{% liquid\n  elsif a\n%}', '{% liquid\n  when a\n%}', '{% for a in b %}{% elsif c %}{% endfor %}', '{% for a in b %}{% when c %}{% endfor %}', '{% if a %}{% when b %}{% endif %}', '{% case a %}{% elsif b %}{% endcase %}', '{% case a %}text{% when b %}c{% endcase %}', '{% case a %}{% when %}{% endcase %}', '{% case a %}{% when , %}{% endcase %}', '{% case a %}{% when b or %}{% endcase %}', '{% case a %}{% when b, [0], c %}{% endcase %}', '{% case a %}{% when b or [0] %}{% endcase %}', '{% capture [0] %}{% endcapture %}', '{% capture a[0] %}{% endcapture %}',
    '{% comment %}{% if %}{% endcomment %}', '{% comment %}{{ a{% endcomment %}', '{% comment %}{% a{% endcomment %}', '{% comment %}{%{% endcomment %}', '{% comment %}{{{% endcomment %}', '{% comment %}{{}}{% endcomment %}', '{% comment %}{%%}{% endcomment %}', '{% comment %}<a href="{{{% endcomment %}', '{% comment %}<a href="{{ a }}">{% endcomment %}', '{% comment %}---\na\n---\n{% endcomment %}', '{% comment %}---\n{{ a\n---\n{% endcomment %}', '{% comment %}\n{% endcomment %}{% comment %}{% endcomment %}',
    '{% comment %}{% comment %}{% endcomment %}', '{% comment %}{% comment %}{% endcomment %}{% endcomment %}', '{% comment %}{% comment %}{% endcomment %}{% endcomment %}{% endcomment %}', '{% comment %}{% endcomment %}{% endcomment %}', '{% comment %}{% comment %}{% endcomment %} {% if a %}{% endif %}', '{% comment %}{% raw %}{% endcomment %}', '{% comment %}{% raw %}{% endraw %}{% endcomment %}', '{% comment %}{% endraw %}{% endcomment %}', '{% comment %}{% raw %}{% endcomment %}{% endraw %}', '{% comment %}{% raw %}{% endcomment %}{% endraw %}{% endcomment %}', '{% comment %}{% comment @ %}{% endcomment %}', '{% comment %}{% endcomment @ %}{% endcomment %}', '{% comment %}{% comment %}{% endcomment @ %}{% endcomment %}{% endcomment %}', '{% comment %}{% raw %}{% endcomment %}{% raw %}{% endraw %}{% endraw %}',
    '{% raw %}{% endraw %}{% endraw %}', '{% comment %}{% raw %}{% endcomment %}{% endraw %}{% comment %}{% endcomment %}', '{% comment %}{% raw %}{% endcomment %}{% endraw %}', '{% raw %}{% comment %}{% endraw %}{% endraw %}{% endcomment %}', '{% comment %}{% raw %}{% endraw %}{% endraw %}{% endcomment %}{% endcomment %}', '{% comment %}{% raw %}x{% endcomment %}y{% endraw %}z{% endcomment %}',
    '{% doc %}{% doc %}{% enddoc %}', '{% doc %}{% doc %}{% enddoc %}{% enddoc %}', '{% doc %}{% doc x %}{% enddoc %}', '{% doc %}{% doc @ %}{% enddoc %}', '{% doc %}{% enddoc x %}{% doc %}{% enddoc %}', '{% doc %}{% enddoc x %}{% doc %}', '{% doc x %}{% enddoc %}', '{% doc @ %}{% enddoc %}', '{% doc %}@param {string} a\n{% enddoc %}', '{% doc %}{{ a {% enddoc %}', '{% doc %}{% if %}{% enddoc %}',
    '{% doc %}😀{% doc %}', '😀{% doc %}{% doc %}😀', '{% doc %}{% enddoc %}{% enddoc %}', '{% enddoc %}', '{% enddoc x %}', '{% doc %}{% enddoc x %}', '{% doc %}{% enddoc x %}{% enddoc %}', '{% doc %}{% enddoc @ %}', '{% doc %}{% enddoc x %}😀',
    '{% javascript x %}', '{% javascript @ %}', '{% javascript %}', '{% endjavascript %}', '{% endjavascript x %}', '{% javascript %}{% endjavascript x %}', '{% javascript %}{% endjavascript @ %}', '{%- javascript x -%}', '{% javascript x %}{% endjavascript x %}', 'a{% javascript %}b', '{% javascript %}{% javascript x %}', '{% javascript %}{% endjavascript %}{% javascript %}', '{% endjavascript %}{% javascript %}',
    '{% raw x %}', '{% raw @ %}{% endraw %}', "{% raw 'x %}{% endraw %}", '{% raw "x" %}{% endraw %}', '{% raw %}{% endraw x %}', '{% raw %}{% endraw x %}{% raw %}', '{% raw %}{% endraw x %}{% raw %}{% endraw %}', '{% raw %}{% endraw x %}{% endraw %}', '{% raw %}{% endraw @ %}', '{% raw %}{% raw %}{% endraw %}', '{% endraw %}', '{% liquid\n  raw\n%}', '{% liquid\n  raw\n  endraw\n%}', '{% liquid\n  raw x\n  endraw\n%}',
    '{% block "a" %}', "{% block 'a' %}", "{% block 'a' %}{% endblock %}{% endblock %}", "{% liquid\n  block 'a'\n%}", "😀{% liquid\n  😀 foo\n  block 'a'\n%}", "{% liquid block 'a' %}", "{% liquid\n\n  if a\n    block 'a'\n  endif\n%}", "{% liquid\n  echo a\n  block 'a'", "{{ a }}\n{% liquid\n  block 'a'\n%}\n{% liquid\n  block 'b'\n%}", "{% if a %}{% liquid\n  block 'a'\n%}{% endif %}", "{% comment %}{% liquid\n  block\n%}{% endcomment %}{% liquid\n  block 'x'\n%}", "{% if\n liquid %}{% liquid\n block 'x'\n%}{% endif %}", "{% liquid\n  blocks\n  block\n%}", "{% liquid\n  xblock\n  block 'x'\n%}",
    "{% partial 'a' %}", "{% liquid\n  partial 'a'\n%}", '{% endpartial %}', "{% partial 'a/b' %}{% endpartial %}", "{% partial '' %}{% endpartial %}", '{% partial a %}{% endpartial %}', "{% partial 'a' @ %}{% endpartial %}",
    '{% layout none %}', "{% layout 'a' %}", '{% layout [0] %}', '{% layout @ %}', '{% sections %}', "{% sections 'a' %}", "{% sections 'a' b %}", '{% sections a %}', "{% sections 'a' @ %}", "{% section 'a' %}", "{% section 'a', b: [0] %}", "{% section 'a', b: [1, 2] %}", "{% section 'a' @ %}", '{% section a %}', '{% section %}',
    "{% form 'a', b %}{% endform %}", "{% form 'a', b, c %}{% endform %}", "{% form 'a', k: 1, c %}{% endform %}", '{% form k: 1 %}{% endform %}', '{% form %}{% endform %}', "{% form 'a', [0] %}{% endform %}", "{% form 'a', k: [0] %}{% endform %}", "{% form 'a' @ %}{% endform %}", "{% form 'a', k: 1 %}{% endform %}",
    '{% theme-check-disable %}{{ @ }}', '{% # theme-check-disable %}{{ @ }}{% # theme-check-enable %}{{ @ }}', '{% # theme-check-disable LiquidSyntaxError %}{{ @ }}', '{% # theme-check-disable Other %}{{ @ }}', '{% comment %}theme-check-disable{% endcomment %}{{ @ }}',
    '---\ntitle: a\n---\n{{ @ }}', '---\n{% if %}\n---\n', '﻿{{ @ }}', 'a\r\nb\r\n{{ @ }}\r\n', '😀\n😀 {{ @ }} 😀\n😀', 'é{{ @ }}é', '<a {{ @ }}>', '<a b={{ @ }}>', '<a b="{{ @ }}">', "<a b='{% if @ %}{% endif %}'>", '<{{ @ }}>a</{{ @ }}>', '<script>{{ @ }}</script>', '<style>{% if @ %}{% endif %}</style>', '<!-- {{ @ }} -->', '<a', '<a b="', '</a>', '<a></b>',
    '{{ a }}{{ @ }}{% assign b = [0] %}{% if @ %}{% elsif [0] %}{% else %}{{ @ }}{% endif %}', '{% for a in b %}{{ @ }}{% else %}{% cycle @ %}{% endfor %}', '{% if a %}{% if b %}{{ @ }}{% endif %}{% endif %}', '{% liquid\n  echo @\n  if @\n    echo [0]\n  elsif [0]\n    nope\n  endif\n  nope2\n%}',
    // Errors whose end can land on the final, non-BMP, character.
    "{% liquid\n  block 'a'\n  endblock\n  block 'b'\n%}", "{% liquid\n  block 'a'\n  endblock\n  partial 'b'\n%}", "{% liquid\n  partial 'a'\n  endpartial\n  partial 'b'\n%}",
    "{% liquid\n  block 'a' 😀", "{% liquid\n  block 😀", "{% liquid\n  raw 😀", "{% liquid\n  partial 'a'\n  😀", '{% block "a" 😀', '{% comment 😀', '{% comment %}😀{% comment %}😀', '{% if a %}😀', '{% endif %}😀', '😀{% endcomment %}😀', '{% endcomment 😀', '{% endraw %}😀', '{{ a 😀', '<a>😀', '<a b="😀', '</a>😀', '😀</a>', '{% raw %}{% endraw 😀', '{% doc %}{% enddoc 😀', '{% javascript %}😀{% endjavascript x %}😀', '{% javascript 😀', '{% endjavascript 😀',
    '{% style %}a{{ @ }}{% endstyle %}', '{% stylesheet %}{{ @ }}{% endstylesheet %}', '{% javascript %}{{ @ }}{% endjavascript %}', '{% style %}{% if %}{% endstyle %}', '{% schema %}{{ @ }}{% endschema %}', '{% stylesheet \'scss\' %}{% endstylesheet %}',
  );
  return out;
}

const SUITES = {
  async themes(suite) {
    for (const [name, root] of Object.entries(THEMES)) {
      for (const file of liquidFiles(root)) {
        await suite.add(`${name}/${path.relative(root, file)}`, fs.readFileSync(file, 'utf8'), [NO_DOCSET], path.relative(root, file));
      }
    }
  },
  async broken(suite) {
    for (const [name, root] of Object.entries(THEMES)) {
      for (const file of liquidFiles(root)) {
        const src = fs.readFileSync(file, 'utf8');
        const rel = path.relative(root, file);
        for (let seed = 1; seed <= SEEDS; seed++) {
          const variants = breakSource(src, hash(rel) + seed);
          for (let v = 0; v < variants.length; v++) {
            await suite.add(`${name}/${rel}#${seed}.${v}`, variants[v], [NO_DOCSET], rel);
          }
        }
      }
    }
  },
  async spec(suite) {
    const specs = [
      path.join(SPEC_DIR, 'index.spec.ts'),
      ...fs.readdirSync(path.join(SPEC_DIR, 'parity')).filter((f) => f.endsWith('.spec.ts')).map((f) => path.join(SPEC_DIR, 'parity', f)),
    ];
    let dropped = 0;
    for (const spec of specs) {
      const ts = fs.readFileSync(spec, 'utf8');
      const constants = {};
      for (const m of ts.matchAll(/^\s*const (\w+) =\s*(['"`])((?:(?!\2).)*)\2;/gms)) constants[m[1]] = m[3];
      const { literals, dropped: d } = literalsOf(ts, constants);
      dropped += d;
      for (const s of literals.filter(isLiquid)) await suite.add(`spec:${s.slice(0, 60)}`, s, [NO_DOCSET, TAGS_DOCSET]);
    }
    const dir = path.join(SPEC_DIR, 'parity/scenarios');
    for (const f of fs.readdirSync(dir).sort()) {
      for (const s of YAML.parse(fs.readFileSync(path.join(dir, f), 'utf8'))) {
        await suite.add(s.id, s.template, [NO_DOCSET, TAGS_DOCSET], s.file_path ?? 'snippets/test.liquid');
        for (let seed = 1; seed <= SEEDS; seed++) {
          for (const v of breakSource(s.template, hash(s.id) + seed)) await suite.add(`${s.id}#${seed}`, v, [NO_DOCSET]);
        }
      }
    }
    suite.note = `${dropped} literal(s) with unresolved interpolation skipped`;
  },
  async handwritten(suite) {
    for (const s of handWritten()) await suite.add(`hand:${s.slice(0, 60)}`, s, [NO_DOCSET, TAGS_DOCSET]);
  },
  async combos(suite) {
    for (const [shape, make] of TAG_SHAPES) {
      for (const fragment of FRAGMENTS) await suite.add(`${shape}:${JSON.stringify(fragment)}`, make(fragment), [NO_DOCSET]);
    }
  },
};

fs.mkdirSync(OUT, { recursive: true });
let failed = 0;
let totalRecords = 0;
let totalOffenses = 0;
for (const [name, run] of Object.entries(SUITES)) {
  if (ONLY && ONLY !== name) continue;
  const suite = new Suite(name);
  await run(suite);
  const file = path.join(OUT, `${name}.jsonl`);
  fs.writeFileSync(file, suite.lines.join('\n') + '\n');
  const res = spawnSync(lqBin(), ['debug-check-golden', file, '--upstream-bugs', '--show', String(SHOW)], { encoding: 'utf8', maxBuffer: 1 << 30 });
  let report;
  try {
    report = JSON.parse(res.stdout);
  } catch {
    console.log(`[${name}] lq crashed: exit ${res.status} signal ${res.signal}\n${res.stderr.slice(0, 2000)}`);
    failed++;
    continue;
  }
  const diffs = report.failed + report.unreplayable + report.skipped;
  failed += diffs;
  totalRecords += report.total;
  totalOffenses += suite.offenses;
  console.log(`[${name}] records=${report.total} oracleOffenses=${suite.offenses} oracleRejections=${suite.rejected} passed=${report.passed} differences=${diffs}${suite.note ? ` (${suite.note})` : ''}`);
  for (const f of report.failures) console.log(`  ${JSON.stringify(f).slice(0, 700)}`);
}
console.log(`[liquid-syntax] records=${totalRecords} oracleOffenses=${totalOffenses} differences=${failed}`);
process.exit(failed ? 1 : 0);
