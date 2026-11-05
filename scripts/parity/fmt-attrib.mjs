// Attribution of embedded-code diffs, shared by fmt-classify.mjs and fmt-fixtures.mjs.
//
// Given a source and both formatters' outputs:
//   - with block bodies blanked, the outputs must be equal: anything else is an integration error;
//   - JSON and pass-through bodies must be equal;
//   - a differing JS/CSS body is attributed to Biome only when, at the block's indent, the oracle's
//     body is Prettier's stand-alone output and lq's body is Biome's stand-alone output, for
//     (width - indent) columns. Anything else counts as an integration error.
import fs from 'node:fs';
import path from 'node:path';
import { GEN, breakSource, oracleRequire, rng, runLq } from './lib.mjs';

const prettier = oracleRequire('prettier');
const { toTolerantLiquidHtmlAST, walk } = oracleRequire('@shopify/liquid-html-parser');
export const PARSER = { css: 'css', javascript: 'babel', typescript: 'typescript', json: 'json' };

export function rawBlocks(src) {
  const blocks = [];
  walk(toTolerantLiquidHtmlAST(src), (n) => {
    if (n.type === 'RawMarkup') blocks.push({ kind: n.kind, value: n.value, start: n.position.start, end: n.position.end });
  });
  return blocks.sort((a, b) => a.start - b.start);
}

function blank(src, blocks) {
  let out = '', last = 0;
  for (const b of blocks) { out += src.slice(last, b.start) + '\u0000'; last = b.end; }
  return out + src.slice(last);
}

const indentOf = (body) => (body.split('\n').find((l) => l.trim() !== '') ?? '').match(/^[ \t]*/)[0];
const stripBody = (body) => body.replace(/^\n/, '').trimEnd();
const dedent = (body, indent) => stripBody(body).split('\n').map((l) => (l.startsWith(indent) ? l.slice(indent.length) : l)).join('\n');
const moduloIndent = (s) => s.split('\n').map((l) => l.trimStart()).join('\n');
const trimmedLines = (s) => s.trim().split('\n').map((l) => l.trim()).join('\n');

/**
 * entries: [{ label, src, oracle, lq, options, count(kind, outcome) }]
 * Returns { outside, integration, examples }.
 */
export async function attribute(entries, name = 'fmt-attrib') {
  const outside = [], integration = [], examples = [], pending = [];
  for (const e of entries) {
    // Both formatters see end-of-line-normalized text.
    const src = rawBlocks(e.src.replace(/\r\n?/g, '\n')).filter((b) => b.value.trim() !== '');
    if (e.oracle === e.lq) {
      for (const b of src) if (PARSER[b.kind]) e.count(b.kind, 'identical');
      continue;
    }
    let ob, gb;
    try { ob = rawBlocks(e.oracle); gb = rawBlocks(e.lq); } catch (err) { outside.push({ e, why: `output does not parse: ${err.message}` }); continue; }
    if (blank(e.oracle, ob) !== blank(e.lq, gb)) { outside.push({ e, why: 'differs outside block bodies' }); continue; }
    const obs = ob.filter((b) => b.value.trim() !== ''), gbs = gb.filter((b) => b.value.trim() !== '');
    if (obs.length !== src.length || gbs.length !== src.length) { outside.push({ e, why: 'block count changed' }); continue; }
    for (const [k, b] of src.entries()) {
      if (!PARSER[b.kind]) {
        if (obs[k].value === gbs[k].value) continue;
        // Markdown has no Rust formatter: lq must print it verbatim, as the plain printer does.
        if (b.kind === 'markdown' && trimmedLines(gbs[k].value) === trimmedLines(b.value)) e.count('markdown', 'passthrough');
        else integration.push({ e, kind: b.kind, why: `plain ${b.kind} block differs`, a: obs[k].value, b: gbs[k].value });
        continue;
      }
      if (obs[k].value === gbs[k].value) { e.count(b.kind, 'identical'); continue; }
      if (b.kind === 'json') { e.count('json', 'integration'); integration.push({ e, kind: 'json', why: 'JSON body differs', a: obs[k].value, b: gbs[k].value }); continue; }
      pending.push({ e, kind: b.kind, value: b.value, oracle: obs[k].value, lq: gbs[k].value });
    }
  }

  const dir = path.join(GEN, `${name}-blocks`);
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  const list = pending.map((p, i) => {
    p.indent = indentOf(p.lq) || indentOf(p.oracle);
    p.indentWidth = p.indent.startsWith('\t') ? p.indent.length * (p.e.options.tabWidth ?? 2) : p.indent.length;
    p.width = Math.max(1, (p.e.options.printWidth ?? 120) - p.indentWidth);
    const file = path.join(dir, `${i}.txt`);
    fs.writeFileSync(file, p.value);
    return JSON.stringify({ file, kind: p.kind, width: p.width, options: p.e.options });
  });
  const listFile = path.join(GEN, `${name}-blocks.list`);
  fs.writeFileSync(listFile, list.join('\n'));
  const alone = pending.length ? runLq(['debug-embed', listFile]).trim().split('\n').map((l) => JSON.parse(l)) : [];
  for (const [i, p] of pending.entries()) {
    // Each side measures from the indent of its own body: a verbatim body keeps source columns.
    const oIndent = indentOf(p.oracle) || p.indent;
    const oWidth = Math.max(1, (p.e.options.printWidth ?? 120) - (oIndent.startsWith('\t') ? oIndent.length * (p.e.options.tabWidth ?? 2) : oIndent.length));
    let oracleAlone = null;
    try {
      const { plugins, parser, ...rest } = p.e.options;
      oracleAlone = (await prettier.format(p.value, {
        ...rest, parser: PARSER[p.kind], printWidth: oWidth, singleQuote: rest.embeddedSingleQuote ?? true,
      })).trimEnd();
    } catch { /* Prettier rejects it: the oracle printed it verbatim */ }
    const biomeAlone = alone[i].error ? null : alone[i].out.trimEnd();
    const oBody = dedent(p.oracle, oIndent), lBody = dedent(p.lq, p.indent);
    const literal = new Set(alone[i].literal ?? []);
    const expectedLq = biomeAlone === null ? null : biomeAlone.split('\n')
      .map((l, n) => (l === '' || literal.has(n) ? l : p.indent + l)).join('\n');
    const lqConsistent = stripBody(p.lq) === expectedLq;
    const fail = (why) => { p.e.count(p.kind, 'integration'); integration.push({ e: p.e, kind: p.kind, why, a: oBody, b: lBody }); };
    if (oracleAlone === null && biomeAlone === null) { fail('both reject but bodies differ'); continue; }
    if (oracleAlone === null) {
      if (lqConsistent && trimmedLines(oBody) === trimmedLines(p.value)) p.e.count(p.kind, 'prettierRejects');
      else fail('prettier rejects, bodies inconsistent');
      continue;
    }
    if (biomeAlone === null) {
      if (moduloIndent(oBody) === moduloIndent(oracleAlone) && trimmedLines(lBody) === trimmedLines(p.value)) p.e.count(p.kind, 'biomeRejects');
      else fail('biome rejects, bodies inconsistent');
      continue;
    }
    if (lqConsistent && moduloIndent(oBody) === moduloIndent(oracleAlone) && oracleAlone !== biomeAlone) {
      p.e.count(p.kind, 'biomeFormat');
      examples.push(p);
    } else {
      fail(`stand-alone mismatch (lqConsistent=${lqConsistent} sameAlone=${oracleAlone === biomeAlone})`);
    }
  }
  return { outside, integration, examples };
}

export function firstDiff(a, b) {
  const x = a.split('\n'), y = b.split('\n');
  let n = 0;
  while (n < x.length && x[n] === y[n]) n++;
  return [x.slice(Math.max(0, n - 1), n + 3), y.slice(Math.max(0, n - 1), n + 3), n + 1];
}

export function printIssue(e) {
  console.log(`  ${e.e.label}: ${e.why}`);
  if (e.a !== undefined) {
    const [x, y, n] = firstDiff(e.a, e.b);
    console.log(`   (line ${n})`);
    console.log(x.map((l) => `    oracle| ${l}`).join('\n'));
    console.log(y.map((l) => `    lq    | ${l}`).join('\n'));
  }
}

// Fragments that reach the parser's error paths and tolerant recovery.
const JUNK = [
  '{% toString %}', '{% valueOf %}', '{% hasOwnProperty %}', '<constructor>', '</constructor>', '<toString a="b"/>', '{% if %}', '{% if a', '{{ a', '{{', '}}', '{%', '%}',
  '{% endif %}', '{% else %}', '{% when a %}', '{% case a %}', '{% for %}', '{% for a in %}', '{% endfor %}', '{% liquid\n if a\n %}', '{% liquid\n  assign a = \n%}', '{% schema %}', '{% endschema %}',
  '{% raw %}', '{% endraw %}', '{% comment %}', '{% endcomment %}', '{% doc %}', '{% enddoc %}', '{% doc %}@param {x [y - z{% enddoc %}', '{% style %}', '{% endstyle %}', '{% javascript %}', '{% endjavascript %}',
  '<script>', '</script>', '<style>', '</style>', '<svg>', '</svg>', '<!--', '-->', '<!-- prettier-ignore -->', '{% # prettier-ignore %}', '<!-- prettier-ignore-attribute -->', '<!-- display: block -->', '<!-- white-space: pre -->',
  '<', '>', '</', '/>', '"', "'", '=', '</div>', '<div', '<{{ a }}>', '</{{ a }}>', '<a-{{ b }} c="d">', '<br/>', '<img>', '</img>', '<p>', '</p>', '<pre>', '</pre>', '<textarea>', '</textarea>',
  '{{ a | b: }}', '{{ a | b: c, }}', '{{ a.b[c]["d"].e? }}', '{{ (1..a) }}', '{{ 1.5 }}', '{{ nil }}', '{{ "a\'b" }}', "{% render 'a' with b as c, d: e %}", "{% section 'a' %}", '{% paginate a by 1, b: c %}{% endpaginate %}', '{% cycle a: b, c %}', '{% block "a", b: [1, c] %}{% endblock %}',
  '\u00A0', '\u2028', '\uFEFF', '\r\n', '\r', '\t', '日本語', '👍', '\n\n\n',
];

// Seeded mutations of a corpus: lib.mjs breakSource, plus a JUNK insertion per file.
export function mutateCases(cases, seed) {
  const out = [];
  cases.forEach((c, i) => {
    const variants = breakSource(c.src, seed * 7919 + i);
    const r = rng(seed * 104729 + i);
    for (let n = 0; n < 2; n++) {
      const at = r(c.src.length + 1);
      variants.push(c.src.slice(0, at) + JUNK[r(JUNK.length)] + c.src.slice(at));
    }
    variants.forEach((s, j) => out.push({ ...c, label: `${c.label}#seed${seed}.${j}`, src: s }));
  });
  return out;
}
