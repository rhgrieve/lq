#!/usr/bin/env node
// Compares `lq debug-ast` with upstream's parser, node by node.
// Usage: ast.mjs [--strict-only|--tolerant-only] [--no-broken] [--limit N] [--show N] [--fuzz N] [--seed N] [--no-lax] [--no-walk]
//
// Besides the themes and their malformed variants, the case list holds the
// inputs of fixtures/error-corpus.ts, Liquid strings harvested from the
// package's ast, tolerant and lax-recover tests, and the traps below. Every case
// runs through toLiquidHtmlAST and toTolerantLiquidHtmlAST, and also toLiquidAST
// and toTolerantLiquidAST unless a --*-only flag is given. Lax recovery helpers
// are compared over the harvested strings.
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { GEN, ROOT, THEMES, breakSource, firstDiff, hash, liquidFiles, oracleRequire, rng, runLq, snippet, writeCases } from './lib.mjs';

const parser = oracleRequire('@shopify/liquid-html-parser');
const args = process.argv.slice(2);
const flag = (f) => args.includes(f);
const opt = (f, d) => (args.includes(f) ? Number(args[args.indexOf(f) + 1]) : d);
const show = opt('--show', 10);
const DROP = new Set(['source', '_source', 'parentNode', 'prev', 'next', 'firstChild', 'lastChild']);
const PKG = path.join(ROOT, 'reference/theme-tools/packages/liquid-html-parser');

const clean = (ast) => JSON.parse(JSON.stringify(ast, (k, v) => (DROP.has(k) ? undefined : v)));
const run = (fn, src) => {
  try {
    return { ast: clean(fn(src)) };
  } catch (e) {
    return { error: { name: e.name, message: e.message, loc: e.loc ?? null } };
  }
};

// ---- Harvesting ----

function unescapeJs(body) {
  return body.replace(/\\(u\{[0-9a-fA-F]+\}|u[0-9a-fA-F]{4}|x[0-9a-fA-F]{2}|\r\n|[\s\S])/g, (_, e) => {
    if (e[0] === 'u' && e[1] === '{') return String.fromCodePoint(parseInt(e.slice(2, -1), 16));
    if ((e[0] === 'u' || e[0] === 'x') && e.length > 1) return String.fromCharCode(parseInt(e.slice(1), 16));
    switch (e) {
      case 'n': return '\n';
      case 't': return '\t';
      case 'r': return '\r';
      case 'b': return '\b';
      case 'f': return '\f';
      case 'v': return '\v';
      case '0': return '\0';
      case '\n': case '\r\n': return '';
      default: return e;
    }
  });
}

// A small TS lexer: yields string literals, and templates as lists of quasis.
function harvestLiterals(code) {
  const strings = [];
  const templates = [];
  let i = 0;
  let prevSignificant = '';

  const skipString = (q) => {
    const start = ++i;
    while (i < code.length && code[i] !== q) i += code[i] === '\\' ? 2 : 1;
    const body = code.slice(start, i);
    i++;
    return unescapeJs(body);
  };

  const readTemplate = () => {
    i++;
    const quasis = [];
    let start = i;
    while (i < code.length && code[i] !== '`') {
      if (code[i] === '\\') {
        i += 2;
      } else if (code[i] === '$' && code[i + 1] === '{') {
        quasis.push(unescapeJs(code.slice(start, i)));
        i += 2;
        scan('}');
        start = i;
      } else {
        i++;
      }
    }
    quasis.push(unescapeJs(code.slice(start, i)));
    i++;
    if (quasis.length === 1) strings.push(quasis[0]);
    else templates.push(quasis);
  };

  function scan(until) {
    let depth = 0;
    while (i < code.length) {
      const c = code[i];
      if (until && c === until && depth === 0) {
        i++;
        return;
      }
      if (c === '{') depth++;
      if (c === '}') depth--;
      if (c === '/' && code[i + 1] === '/') {
        while (i < code.length && code[i] !== '\n') i++;
      } else if (c === '/' && code[i + 1] === '*') {
        i = code.indexOf('*/', i) + 2;
      } else if (c === '/' && /[(,=:\[!&|?{};\n]|^$|return$/.test(prevSignificant)) {
        // A regex literal.
        i++;
        let inClass = false;
        while (i < code.length && (code[i] !== '/' || inClass) && code[i] !== '\n') {
          if (code[i] === '\\') i++;
          else if (code[i] === '[') inClass = true;
          else if (code[i] === ']') inClass = false;
          i++;
        }
        i++;
        prevSignificant = ')';
      } else if (c === '"' || c === "'") {
        strings.push(skipString(c));
        prevSignificant = ')';
      } else if (c === '`') {
        readTemplate();
        prevSignificant = ')';
      } else {
        if (/\S/.test(c)) {
          // Keep enough of a word to recognise `return`.
          prevSignificant = /\w/.test(c) ? (/\w$/.test(prevSignificant) ? prevSignificant + c : c) : c;
        }
        i++;
      }
    }
  }

  scan();
  return { strings, templates };
}

function harvestTests() {
  const files = ['ast.test.ts', 'tolerant.test.ts', 'lax-recover.test.ts'];
  const strings = new Set();
  const templates = [];
  for (const f of files) {
    const lits = harvestLiterals(fs.readFileSync(path.join(PKG, 'src', f), 'utf8'));
    lits.strings.forEach((s) => strings.add(s));
    templates.push(...lits.templates);
  }
  const liquidish = (s) => /\{\{|\{%|<|%\}|\}\}|>/.test(s);
  const whole = [...strings].filter(liquidish);
  // Table entries such as `expression: 'a.b'` are spliced into templates like
  // `{{ ${expression} }}` by the tests, so do the same.
  const plain = [...strings].filter((s) => !liquidish(s) && s.length > 0 && s.length < 80);
  const spliced = new Set();
  for (const quasis of templates) {
    const joined = quasis.join('');
    if (!liquidish(joined)) continue;
    if (quasis.length === 2) {
      plain.forEach((p) => spliced.add(quasis[0] + p + quasis[1]));
    } else {
      spliced.add(quasis.join('x'));
    }
  }
  return { whole, spliced: [...spliced].filter((s) => !strings.has(s)), plain };
}

function errorCorpus() {
  const code = fs
    .readFileSync(path.join(PKG, 'fixtures/error-corpus.ts'), 'utf8')
    .replace(/export const ERROR_FILES:[^=]*=/, 'ERROR_FILES =');
  const context = {};
  vm.runInNewContext(code, context);
  return context.ERROR_FILES.map((f) => ({ label: f.path, src: f.source }));
}

// Inputs for the JS semantics that are easy to get wrong in a port.
const TRAPS = [
  '', ' ', '\n', ' ', '﻿', '﻿<a></a>', '{{', '{%', '}}', '%}', '{{}}', '{%%}', '{%-%}', '{{-}}', '{{--}}', '{%--%}',
  // Object.prototype members are "literals" and "tags".
  '{{ constructor }}', '{{ toString }}', '{{ __proto__ }}', '{{ hasOwnProperty.x }}', '{{ valueOf[0] }}',
  '{% constructor %}', '{% toString %}', '{% __proto__ %}', '{% endconstructor %}', '{% endtoString %}',
  '{% liquid\n  toString\n%}', '{% liquid\n  __proto__ x\n  echo 1\n%}', '<a {% valueOf %}></a>', '{% if a %}{% toString %}{% endif %}',
  // Whitespace beyond ASCII.
  '{{ a }}', '{% if a %}x{% endif %}', '{{﻿a |　b }}', '<a b="c" d></a>', '<a b = "c"></a>',
  '<div﻿>x</div﻿>', '<script></script marker></script>', '<svg></svg >', '  text  ', '<a> </a>', '{% if a %} {% endif %}',
  '{% raw %}x{% endraw %}', '{% comment %}{% endcomment -%}', '{% liquid\n echo a \n \n%}',
  '{% liquid\n  comment \n  x\n  endcomment\n%}', '{% liquid\n  raw \n  x \n  endraw \n  echo 1\n%}', '{% liquid\n  echo a\u0085\n%}', '\u0085<a>\u0085</a>', '<a\u0085b></a>', '{{\u0085a}}',
  // Multi-byte text and positions.
  'é{{ a }}é', '😀{{ "😀" }}😀', '<a b="😀">😀</a>', '{% if a %}é{% else %}😀{% endif %}', '{{ é }}', '{{ a | é }}', '{{ a😀b }}', '<é>x</é>', '<aé b>x</aé>',
  '{% liquid\n  echo "é"\n  assign x = "😀"\n  if x\n    echo x\n  endif\n%}', '{% é %}', '{% é x %}', '{%😀%}', 'é\n😀\n{% endfor %}', 'é\n😀\n</a>', '😀{% if %}', '<a>😀', '😀',
  '{% doc %}é @param {é} é - é\n @example é\n😀 @prompt 😀{% enddoc %}', '{% doc %}@param😀{% enddoc %}', '{% doc %}😀@param x{% enddoc %}', '{% doc %} é@é @x{% enddoc %}',
  // Curly quotes.
  '<a b=“c d”>x</a>', '<a b=”c d“>x</a>', '<a b=‘c d’>x</a>', '<a b=’c d‘>x</a>', '<a b=“c d“>x</a>', '<a b=“c"d”>x</a>', '<a b="c“d">x</a>', '<a b=“{{ c }} {% if d %}e{% endif %}”>x</a>',
  '<a “b”>x</a>', '<a b=“c>x</a>', '<script type=“application/json”>{}</script>', '<a b=“{% raw %}{{{% endraw %}”>x</a>',
  // Case folding beyond ASCII: U+212A KELVIN SIGN lowercases to k.
  '<Keygen>', '<linK>', '<SCRIPT></script>', '<script></SCRIPT>', '<Svg></sVG>', '<BR>', '<INPUT a=b>', '<STYLE>{{ a }}</STYLE>', '<script TYPE="application/ld+json"></script>', '<script type=text/html></script>',
  // Reslicing: a stray opener in a raw body, in each position.
  '{% raw %}{{{% endraw %}{{ 1 }}', '{% raw %}{%{% endraw %}{{ 1 }}', '<a b="{% raw %}{{{% endraw %}">x</a>', "<a b='{% raw %}{%{% endraw %}'>x</a>", '<a {% raw %}{{{% endraw %} c>ok</a>', '<a {% raw %}{%{% endraw %}>ok</a>', '</a {% raw %}{{{% endraw %}>',
  '<!doctype {% raw %}{{{% endraw %}>', '{% comment %}{{{% endcomment %}<a>x</a>', '{% schema %}{%{% endschema %}---\n', '{% style %}{{{% endstyle %}x', '{% style %}{% if %}{% endstyle %}x', '{% javascript %}{{ a {% endjavascript %}{{ b }}', '{% javascript %}{% raw %}{{{% endraw %}{% endjavascript %}',
  '<script>{% raw %}{{{% endraw %}</script>', '<script>{% if x %}</script>{% endif %}</script>', '<script>{{</script>', '<script>{%</script>x', '<style>{% endif %}</style><a>x</a>', '<style>{% endif %}</style></b>', '<svg><svg></svg></svg>', '<svg><svg></svg>', '<script><script></script></script>',
  // State left behind by a throw in tolerant mode.
  '<script>{% if a %}</script><a>b</a>', '<a b="{% if c %}{% endif"><a>d</a>', '<a b="{% endif %}"><a c="{% if d %}e"></a></a>', '<a {% if b %}{% endfor %}{% endif %}>c</a><d>e</d>', '{% if a %}<b>{% endfor %}</b>{% endif %}<c>d</c>', '{% for a in b %}</c>{% endfor %}</d>',
  // Blocks, branches, stalls.
  '{% if a %}{% endwhen %}{% endif %}', '{% capture a %}{% endwhen %}{% endcapture %}', '{% capture a %}{% else %}{% endcapture %}', '{% case a %}{% endwhen %}{% endcase %}', '{% case a %}x{% when 1 %}y{% else %}z{% endcase %}', '{% for a in b %}x{% else %}y{% endfor %}', '{% for a in b %}{% elsif c %}{% endfor %}', '{% if a %}{% when b %}{% endif %}',
  '{% if %}{% endif %}', '{% if a b %}{% elsif %}{% elsif c d %}{% else x %}{% endif %}', '{% unless a %}{% endunless %}', '{% case %}{% when %}{% endcase %}', "{% case a %}{% when 'b': %}{% endcase %}", '{% case a %}{% when 1 or 2, 3 %}{% endcase %}', '{% form %}{% endform %}', "{% form 'a', b: c %}{% endform %}", '{% paginate a by b c: d %}{% endpaginate %}', '{% paginate a by b, c: d, %}{% endpaginate %}',
  "{% block 'a', b: [1, 'c', d], %}{% endblock %}", "{% block 'a', b: [[1]] %}{% endblock %}", "{% block 'shopify://apps/a/blocks/b/00000000-0000-0000-0000-000000000000' %}{% endblock %}", "{% block 'shopify://apps/a/blocks/b/0' %}{% endblock %}", "{% block '_a-b' %}{% endblock %}", "{% block '_' %}{% endblock %}", "{% block 'a\n' %}{% endblock %}", '{% block %}{% endblock %}', '{% block a %}{% endblock %}', "{% block 'a' x %}{% endblock %}", "{% section 'a', b: [1] %}", "{% section 'a', %}", '{% section a %}',
  "{% render 'a' with b as c, d: e f: g %}", "{% render 'a' for b %}", '{% render 1 %}', '{% include 1 %}', '{% include nil %}', "{% render 'a', b.c: d %}", "{% content_for 'a', b: c d: e, f %}", '{% content_for a %}', '{% cycle a: b, c %}', '{% cycle a, %}', '{% for a in b reversed c: d, e: f %}{% endfor %}', '{% for a in b, %}{% endfor %}', '{% tablerow a in (1..b) %}{% endtablerow %}', '{% assign a == b %}', '{% assign a = %}', '{% assign = b %}', '{% echo %}', '{% echo a | b: c, d: e | f %}', '{% break x %}', '{% continue %}', '{% layout none %}', '{% sections "a" %}', '{% sections a %}', '{% increment a %}', '{% decrement a.b %}', '{% ifchanged %}{% endifchanged %}', '{% ifchanged a %}{% endifchanged %}', "{% partial 'a' %}{% endpartial %}", '{% partial a %}{% endpartial %}',
  '{% # comment %}', '{%#comment%}', '{%- # a -%}', '{% #a  b %}', '{% liquid\n# a\n  #  b\n#\tc\n#\n%}', '{% endfoo %}', '{% end %}', '{% if a %}{% endfoo %}{% endif %}', '<a>{% endfoo %}</a>', '<a b="{% endfoo %}"></a>', '<a b="{% else %}"></a>', '<{% if a %}b{% else %}c{% endif %}>d</{% if a %}b{% else %}c{% endif %}>', '<{{ a }}{% if b %} c{% endif %}>d</{{ a }}>', '<li{% if a %} b{% endif %}>c</li>', '<c-{% if a %}{{ b }}{% endif %}>d</c-{% if a %}{{ b }}{% endif %}>', '<li{% if a -%} b{% endif %}>c</li>', '<li{% if a %} {{- b }}{% endif %}>c</li>', '<li{% echo a %}>b</li{% echo a %}>', '<li{% case a %}{% when b %} c{% endcase %}>d</li>', '<a{% raw %} {% endraw %}>b</a>',
  // {% liquid %} lines.
  "{% liquid\n  render 'a',\n    b: c,\n    d: e\n  echo f\n%}", '{% liquid\n  render "a",\n  echo b\n%}', '{% liquid\n  if a\n    echo b\n  elsif c d\n  else\n    echo e\n  endif\n%}', '{% liquid\n  if a\n%}', '{% liquid\n  for a in b\n    break\n%}', '{% liquid\n  case a\n    when 1\n      echo 2\n  endcase\n%}', '{% liquid\n  endif\n%}', '{% liquid\n  comment\n    comment\n    endcomment\n    raw\n    endcomment\n    endraw\n  endcomment\n  echo a\n%}', '{% liquid\n  comment\n%}', '{% liquid\n  raw\n%}', '{% liquid\n  javascript\n  a\n  endjavascript\n%}', '{% liquid\n  doc\n    @param a\n  enddoc\n%}', '{% liquid\n  liquid echo a\n%}', '{% liquid\r\n  echo a\r\n  if b\r\n  endif\r\n%}', '{% liquid %}', '{% liquid\n%}', '{%- liquid -%}', '{% liquid echo a %}', '{% liquid\n  capture a\n    echo b\n  endcapture\n  capture\n  endcapture\n  capture a\n%}', '{% liquid\n  else\n%}', '{% liquid\n  when\n%}',
  // Doc.
  '{% doc %}{% enddoc %}', '{% doc %}\n\n{% enddoc %}', '{% doc %} a\n\n  b\n@param c\n{% enddoc %}', '{% doc %}@param{% enddoc %}', '{% doc %}@param {a} [b] - c\n  d\n\n  e\n@example\n\n  f\n  g\n\n@description h\ni\n@prompt\n j\n@unknown k\n@descriptionText l\nm\n{% enddoc %}', "{% doc %}@param {'a}' b} c{% enddoc %}", '{% doc %}@param {"a c{% enddoc %}', '{% doc %}@param {a c{% enddoc %}', '{% doc %}@param [a c{% enddoc %}', '{% doc %}@param - a{% enddoc %}', '{% doc %}@param -{% enddoc %}', '{% doc %}@param -\n  a{% enddoc %}', '{% doc %}@param a -b{% enddoc %}', '{% doc %}@param !{% enddoc %}', '{% doc %}\r\n@param a\r\n\r\nb\r\n@example\r\nc\r\n{% enddoc %}', '{% doc %}a @b c{% enddoc %}', '{% doc %}a\r@b{% enddoc %}', '{% doc %}a @b{% enddoc %}', '{% doc %}  @description{% enddoc %}', '{% doc %}@description\n{% enddoc %}', '{% doc %}@example{% enddoc %}', '{% doc %}@prompt{% enddoc %}', '{% doc %}@description\n\n@example\n{% enddoc %}', '{% doc %}{% doc %}{% enddoc %}{% raw %}{% enddoc %}{% endraw %}{% enddoc %}', '{% doc %}{% comment %}{% enddoc %}', '{% doc x %}{% enddoc x %}', '{% comment %}{% comment %}{% endcomment %}{% endcomment %}x', '{% comment %}{% commentx %}{% endcomment %}', '{% comment %}{% raw %}{% endcomment %}{% endraw %}{% endcomment %}', '{% comment %}{% raw %}{% endcomment %}', '{%comment%}{%-endcomment-%}', '{%raw%}{%-endraw-%}', '{% raw %}{%endrawx%}{% endraw %}', '{% raw %}{%- - endraw %}{% endraw %}',
  // HTML odds and ends.
  '<!doctype html>', '<!DOCTYPE HTML PUBLIC "a">', '<!doctype\nhtml\n"b"\n"c">', '<!a>', '<!>', '<!doctype html', '<!-- a -->', '<!-- a', '-->', '<!--[if IE]><a><![endif]-->', '<!--[if IE]>x<![endif]-->y', '<![endif]>', '< a', '</', '</ a>', '<a', '<a b', '<a b=', '<a b="', "<a b='", '<a b=c', '<a b=c/>', '<a b=c />', '<a/b>', '<a / b>', '<a =b>', '<a ==b>', '<a b==c>', '<a "b">', '<a b = "c" d = e f = {{ g }}h>x</a>', '<a {{ b }}="c">x</a>', '<a b{{ c }}=d>x</a>', '<a {% if b %}c="d"{% endif %} e>x</a>', '<a {% if b %}c{% else %}d{% endif %}>x</a>', '<a {% if b %}>c</a>', '<a b="{% if c %} d {% else %} e {% endif %}">x</a>', '<a b={% if c %}d{% endif %}>x</a>', '<a b={{ c }}{% if d %}e{% endif %}>x</a>', '<a/>', '<a />', '<a></b>', '</a>', '<a></a >', '<a></a b>', '<a></a\n>', '<br/>', '<br>x</br>', '<a>{% if b %}</a>{% endif %}', '{% if b %}<a>{% endif %}', '{% if b %}</a>{% endif %}', '{% unless b %}<a>{% else %}</a>{% endunless %}', '{% for b in c %}<a>{% endfor %}', '{% capture b %}<a>{% endcapture %}', '<a>{% for b in c %}</a>{% endfor %}', '---\na: b\n---\n<a>', '---\r\na: b\r\n---\r\nc', '---\n---\n', '---\n\n---', '---\na', '---', ' ---\na\n---\n', '---\n---\n---\n', '---a\n---\n',
  // Markup expressions.
  '{{ a.b["c"][d].e? }}', '{{ a. }}', '{{ .a }}', '{{ a..b }}', '{{ [a] }}', '{{ ["a"].b }}', '{{ [] }}', '{{ [a }}', '{{ (1..2) }}', '{{ (a.b..c[d]) }}', '{{ (1...2) }}', '{{ (1..2 }}', '{{ 1.5 }}', '{{ -1 }}', '{{ - 1 }}', '{{ 1_000.5_0 }}', '{{ 1. }}', '{{ .5 }}', '{{ 1.2.3 }}', '{{ a-b }}', '{{ a? }}', '{{ a?b }}', '{{ "a }}', '{{ \'a }}', '{{ "a\'b" }}', '{{ nil }}', '{{ null.a }}', '{{ true[0] }}', '{{ blank }}', '{{ empty }}', '{{ contains }}', '{{ a.contains }}', '{{ a.and.or }}', '{{ and }}', '{{ a and b }}', '{{ a or b and c }}', '{{ a == b }}', '{{ a = b }}', '{{ a != b }}', '{{ a <> b }}', '{{ a contains b | c }}', '{{ a | b | c: d, e: f }}', '{{ a | }}', '{{ a | | b }}', '{{ a | b: }}', '{{ a | b: | c }}', '{{ a | b:, }}', '{{ a | b: c, }}', '{{ a | b: ! }}', '{{ a | b: c.d: e }}', '{{ a b }}', '{{ a !b }}', '{{ a,b }}', '{% if a and b or c == d %}{% endif %}', '{% if a and %}{% endif %}', '{% if a == %}{% endif %}', '{% if a = b %}{% endif %}', '{% if (a) %}{% endif %}', '{% if a  and  b  %}{% endif %}', '{% elsif a %}', '{% else %}', '{% when a %}',
];

// ---- Cases ----

const real = Object.values(THEMES).flatMap(liquidFiles);
const sources = real.map((f) => ({ label: f, src: fs.readFileSync(f, 'utf8') }));
const cases = [...sources];
if (!flag('--no-broken')) {
  for (const { label, src } of sources) {
    breakSource(src, hash(label)).forEach((s, i) => cases.push({ label: `${label}#broken${i}`, src: s }));
  }
}
const themeCount = cases.length;
const corpus = errorCorpus();
const harvested = harvestTests();
cases.push(...corpus);
cases.push(...harvested.whole.map((src, i) => ({ label: `test-string#${i}: ${JSON.stringify(src).slice(0, 60)}`, src })));
cases.push(...harvested.spliced.map((src, i) => ({ label: `test-template#${i}: ${JSON.stringify(src).slice(0, 60)}`, src })));
cases.push(...TRAPS.map((src, i) => ({ label: `trap#${i}: ${JSON.stringify(src).slice(0, 60)}`, src })));

// Optional: heavy mutation of the small cases and of chunks of the themes.
const fuzzN = opt('--fuzz', 0);
if (fuzzN > 0) {
  const r = rng(opt('--seed', 1));
  const JUNK = ['{%', '%}', '-%}', '{%-', '{{', '}}', '-}}', '{{-', '<', '>', '/>', '</', '"', "'", '=', '</a>', '<a ', '<script>', '</script>', '<!--', '-->', '<!', '{% endif %}', '{% if a %}', '{% else %}', '{% raw %}', '{% endraw %}', '{% comment %}', '{% endcomment %}', '{% liquid\n', '\n', '\r\n', ' ', ' ', '﻿', 'é', '😀', '“', '”', '’', '|', ':', ',', '.', '..', '(', ')', '[', ']', '-', '@param ', '{% doc %}', '{% enddoc %}', 'end', 'constructor', '---\n'];
  const seeds = [...corpus.map((c) => c.src.slice(0, 400)), ...harvested.whole, ...TRAPS];
  for (const { src } of sources) {
    for (let k = 0; k < 3; k++) {
      const at = r(Math.max(1, src.length));
      seeds.push(src.slice(at, at + 200 + r(2000)));
    }
  }
  for (let n = 0; n < fuzzN; n++) {
    let s = seeds[r(seeds.length)];
    for (let m = 1 + r(4); m > 0; m--) {
      const at = r(s.length + 1);
      switch (r(4)) {
        case 0: s = s.slice(0, at) + JUNK[r(JUNK.length)] + s.slice(at); break;
        case 1: s = s.slice(0, at) + s.slice(at + 1 + r(8)); break;
        case 2: { const o = seeds[r(seeds.length)]; s = s.slice(0, at) + o.slice(0, 60) + s.slice(at); break; }
        default: { const j = r(s.length + 1); s = s.slice(0, Math.min(at, j)) + s.slice(Math.max(at, j)); }
      }
    }
    cases.push({ label: `fuzz#${n}: ${JSON.stringify(s).slice(0, 120)}`, src: s });
  }
}

console.log(
  `[ast] cases: ${themeCount} theme (${sources.length} files + malformed variants), ` +
    `${corpus.length} error-corpus, ${harvested.whole.length} test strings, ` +
    `${harvested.spliced.length} test templates, ${TRAPS.length} traps` +
    (fuzzN ? `, ${fuzzN} fuzz` : ''),
);

const limit = opt('--limit', cases.length);
// A case is what a file on disk can hold: slicing a surrogate pair leaves a lone
// surrogate, which UTF-8 encoding replaces. Both sides parse those same bytes.
const selected = cases.slice(0, limit).map((c) => ({ label: c.label, src: c.src.toWellFormed() }));
const files = writeCases(path.join(GEN, 'ast-cases'), selected.map((c) => c.src));
const listFile = path.join(GEN, 'ast-cases.list');
fs.writeFileSync(listFile, files.join('\n'));

let failed = false;
const modes = [];
if (!flag('--tolerant-only')) modes.push(['strict', parser.toLiquidHtmlAST, []]);
if (!flag('--strict-only')) modes.push(['tolerant', parser.toTolerantLiquidHtmlAST, ['--tolerant']]);
if (!flag('--tolerant-only') && !flag('--strict-only')) {
  modes.push(['liquid-only', parser.toLiquidAST, ['--liquid-only']]);
  modes.push(['tolerant-liquid-only', parser.toTolerantLiquidAST, ['--tolerant', '--liquid-only']]);
}

const report = (name, same, total, detail, diff, shown) => {
  console.log(`[${name}] ${same}/${total} match${detail}, ${diff} differ`);
  if (shown.length) console.log(shown.join('\n'));
  if (diff) failed = true;
};

for (const [mode, fn, lqArgs] of modes) {
  const lines = runLq(['debug-ast', ...lqArgs, '--files-from', listFile]).trim().split('\n');
  let same = 0, diff = 0, okBoth = 0, errBoth = 0;
  const shown = [];
  selected.forEach((c, i) => {
    const want = run(fn, c.src);
    const got = JSON.parse(lines[i]);
    // Rust reports the error location it computed; message and position must both match.
    const d = firstDiff(want, { ast: got.ast, error: got.error }.ast !== undefined ? { ast: got.ast } : { error: got.error });
    if (d) {
      diff++;
      if (shown.length < show) shown.push(`  ${c.label}\n    ${d.path}\n    oracle: ${snippet(d.oracle)}\n    lq:     ${snippet(d.lq)}`);
    } else {
      same++;
      if (want.ast) okBoth++; else errBoth++;
    }
  });
  report(`ast/${mode}`, same, selected.length, ` (${okBoth} parsed, ${errBoth} same error)`, diff, shown);
}

// ---- walk order and node ids ----

if (!flag('--no-walk')) {
  const oracleWalk = (src) => {
    let ast;
    try {
      ast = parser.toTolerantLiquidHtmlAST(src);
    } catch {
      return null;
    }
    const out = [];
    parser.walk(ast, (n) => out.push([n.type, n.position.start, n.position.end]));
    return { walk: out, idsDense: true };
  };
  const lines = runLq(['debug-ast', '--tolerant', '--walk', '--files-from', listFile]).trim().split('\n');
  let same = 0, diff = 0;
  const shown = [];
  selected.forEach((c, i) => {
    const want = oracleWalk(c.src);
    const got = JSON.parse(lines[i]);
    const d = want === null ? (got.error ? null : { path: '$', oracle: 'throws', lq: 'parsed' }) : firstDiff(want, got);
    if (d) {
      diff++;
      if (shown.length < show) shown.push(`  ${c.label}\n    ${d.path}\n    oracle: ${snippet(d.oracle)}\n    lq:     ${snippet(d.lq)}`);
    } else same++;
  });
  report('walk', same, selected.length, '', diff, shown);
}

// ---- Lax recovery helpers ----

if (!flag('--no-lax') && !flag('--strict-only') && !flag('--tolerant-only')) {
  const inputs = [...new Set([...harvested.plain, ...harvested.whole.filter((s) => s.length < 200), ...harvested.spliced.filter((s) => s.length < 80)])].filter((s) => !s.includes('\n') || s.length < 200);
  const tags = ['assign', 'block', 'break', 'capture', 'case', 'content_for', 'continue', 'cycle', 'decrement', 'echo', 'for', 'form', 'if', 'ifchanged', 'include', 'increment', 'layout', 'liquid', 'paginate', 'partial', 'render', 'section', 'sections', 'tablerow', 'unless', 'comment', 'nosuchtag', 'toString'];
  const laxCases = [];
  for (const s of inputs) {
    const m = s.toWellFormed();
    laxCases.push({ kind: 'variable', tag: '', m });
    laxCases.push({ kind: 'when', tag: '', m });
    for (const tag of tags) laxCases.push({ kind: 'tag', tag, m });
  }
  const laxFile = path.join(GEN, 'lax-cases.txt');
  fs.writeFileSync(laxFile, laxCases.map((c) => `${c.kind}\t${c.tag}\n${c.m}`).join('\0'));
  const lines = runLq(['debug-lax', '--cases', laxFile]).trim().split('\n');
  const oracleLax = (c) => {
    try {
      let v;
      if (c.kind === 'variable') v = parser.laxRecoverVariable(c.m, c.m, 0);
      else if (c.kind === 'when') v = parser.laxRecoverWhenValues(c.m, c.m, 0, c.m.length);
      else v = parser.laxRecoverTagMarkup(c.tag, c.m, c.m, 0, c.m.length);
      return v === undefined ? { undefined: true } : { value: clean(v) };
    } catch (e) {
      return { error: { name: e.name, message: e.message, loc: e.loc ?? null } };
    }
  };
  let same = 0, diff = 0;
  const shown = [];
  laxCases.forEach((c, i) => {
    const d = firstDiff(oracleLax(c), JSON.parse(lines[i]));
    if (d) {
      diff++;
      if (shown.length < show) shown.push(`  ${c.kind} ${c.tag} ${JSON.stringify(c.m).slice(0, 100)}\n    ${d.path}\n    oracle: ${snippet(d.oracle)}\n    lq:     ${snippet(d.lq)}`);
    } else same++;
  });
  report('lax', same, laxCases.length, '', diff, shown);
}

process.exit(failed ? 1 : 0);
