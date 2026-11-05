#!/usr/bin/env node
// Differential test of lq_json::validator against theme-check's JSONValidator.
// Usage: jsonschema.mjs [--mutations N] [--synth-batches N] [--show N] [--seed N] [--only group,group]
//
// Needs `cargo build -p lq-json --release` (or LQ_JSON_DEBUG=<binary>).
//
// Groups:
//   real        every {% schema %} body and templates/config/locales JSON of the themes
//   mutated     seeded structural and textual mutations of those documents
//   settings    generated settings of every setting type, with random fields
//   order       the same documents with each schema entry point resolved first
//   traps       hand-written edge cases, including deep nesting
//   handwritten focused schemas: best match, enum merging, discriminators, $ref, messages
//   synth       random schemas exercising every keyword, with generated documents
//   url         `new URL` against the Rust port
//
// Named categories are comparisons that cannot be exact, and are always printed:
//   raw-lone-surrogate    the text holds an unpaired UTF-16 unit (no Rust String can)
//   escaped-lone-surrogate a string value holds one through \uD8xx; values use U+FFFD
//   mid-surrogate-offset  an offset inside an astral character (no byte offset exists)
//   url-idna              `new URL` on a non-ASCII host, which needs IDNA
//
// Not compared: nesting between ~500 and ~5000 levels (see trapDocuments), and external
// `$ref` cycles through unresolved sections, which upstream never settles.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { GEN, ROOT, THEMES, hash, liquidFiles, oracleRequire, rng, snippet } from './lib.mjs';

const { JSONValidator } = oracleRequire('@shopify/theme-check-common');
const { toLiquidHtmlAST } = oracleRequire('@shopify/liquid-html-parser');
const jsonc = oracleRequire('jsonc-parser');

const args = process.argv.slice(2);
const opt = (f, d) => (args.includes(f) ? Number(args[args.indexOf(f) + 1]) : d);
const MUTATIONS = opt('--mutations', 24);
const SYNTH_BATCHES = opt('--synth-batches', 300);
const SHOW = opt('--show', 12);
const SEED = opt('--seed', 1);
const only = args.includes('--only') ? new Set(args[args.indexOf('--only') + 1].split(',')) : null;
const enabled = (g) => !only || only.has(g);

const DOCS = path.join(ROOT, 'data/docs');
const BIN = process.env.LQ_JSON_DEBUG ?? path.join(ROOT, 'target/release/lq-json-debug');
const SCHEMA_ROOT = 'https://raw.githubusercontent.com/Shopify/theme-liquid-docs/main/schemas';
const OUT = path.join(GEN, 'jsonschema');
fs.mkdirSync(OUT, { recursive: true });
const DIFF_FILE = path.join(OUT, 'diffs.txt');
fs.writeFileSync(DIFF_FILE, '');

// ---- Running both sides ----

function loadSchemas(docs, manifestFile) {
  const manifest = JSON.parse(fs.readFileSync(manifestFile, 'utf8'));
  return manifest.schemas.map((s) => ({
    uri: `${SCHEMA_ROOT}/${s.uri}`,
    fileMatch: s.fileMatch,
    schema: fs.readFileSync(path.join(docs, path.basename(s.uri)), 'utf8'),
  }));
}

const errorOf = (e) => ({ name: e?.name ?? String(e), message: e?.message ?? String(e) });

async function runOracle(mode, docs, manifestFile, cases) {
  const schemas = loadSchemas(docs, manifestFile);
  const validator = await JSONValidator.create({ schemas: async () => schemas }, { context: mode });
  const { TextDocument } = oracleRequire('vscode-languageserver-textdocument');
  const out = [];
  for (const c of cases) {
    const result = {};
    try {
      const diagnostics = await validator.validate(c.uri, c.json);
      const doc = TextDocument.create(c.uri, 'json', 0, c.json);
      const raw = await validator.service.doValidation(doc, validator.service.parseJSONDocument(doc), {
        schemaValidation: 'error',
        trailingCommas: 'ignore',
        comments: 'ignore',
      });
      result.diagnostics = diagnostics.map((d, i) => ({
        message: d.message,
        start: d.startIndex,
        end: d.endIndex,
        severity: raw[i].severity,
      }));
    } catch (e) {
      result.error = errorOf(e);
    }
    try {
      result.isValid = await validator.isValid(c.uri, c.json);
    } catch (e) {
      result.isValidError = errorOf(e);
    }
    out.push(result);
  }
  return out;
}

let batchId = 0;
function runBin(mode, docs, manifestFile, cases) {
  const id = batchId++ % 8;
  const casesFile = path.join(OUT, `cases-${id}.ndjson`);
  const outFile = path.join(OUT, `out-${id}.ndjson`);
  const fd = fs.openSync(casesFile, 'w');
  for (const c of cases) fs.writeSync(fd, `${JSON.stringify({ uri: c.uri, json: c.json })}\n`);
  fs.closeSync(fd);
  const out = fs.openSync(outFile, 'w');
  execFileSync(BIN, ['validate', '--docs', docs, '--mode', mode, '--manifest', manifestFile, '--cases', casesFile], {
    stdio: ['ignore', out, 'inherit'],
  });
  fs.closeSync(out);
  const lines = fs.readFileSync(outFile, 'utf8').split('\n');
  lines.pop();
  return lines.map((l) => JSON.parse(l));
}

// ---- Coverage: which upstream behaviours the run actually hit ----

const TEMPLATES = [
  /^Incorrect type\. Expected one of /, /^Incorrect type\. Expected "/, /^Matches a schema that is not allowed\.$/, /^Matches multiple schemas when only one must validate\.$/,
  /^Value is not accepted\. Valid values: /, /^Value must be /, /^Value is deprecated$/, /^Value is not divisible by /, /^Value is below the exclusive minimum of /,
  /^Value is above the exclusive maximum of /, /^Value is below the minimum of /, /^Value is above the maximum of /, /^String is shorter than the minimum length of /, /^String is longer than the maximum length of /,
  /^String does not match the pattern of "/, /^String is not a URI: URI expected\.$/, /^String is not a URI: URI with a scheme is expected\.$/, /^Invalid color format\./, /^String is not a RFC3339 date-time\.$/,
  /^String is not a RFC3339 date\.$/, /^String is not a RFC3339 time\.$/, /^String is not an e-mail address\.$/, /^String is not a hostname\.$/, /^String is not an IPv4 address\.$/, /^String is not an IPv6 address\.$/,
  /^Array has too many items according to schema\./, /^Array does not contain required item\.$/, /^Array has too few items that match the contains contraint\./, /^Array has too many items that match the contains contraint\./,
  /^Item does not match any validation rule from the array\.$/, /^Array has too few items\. /, /^Array has too many items\. /, /^Array has duplicate items\.$/, /^Missing property "/, /^Property .* is not allowed\.$/,
  /^Object has more properties than limit of /, /^Object has fewer properties than the required number of /, /^Object is missing property /,
  /^Invalid unicode sequence in string\.$/, /^Invalid escape character in string\.$/, /^Unexpected end of number\.$/, /^Unexpected end of comment\.$/, /^Unexpected end of string\.$/, /^Invalid characters in string\./,
  /^Value expected$/, /^Expected comma$/, /^Expected comma or closing bracket$/, /^Property keys must be doublequoted$/, /^Duplicate object key$/, /^Colon expected$/, /^Property expected$/, /^Expected comma or closing brace$/,
  /^Expected a JSON object, array or literal\.$/, /^End of file expected\.$/, /^\$ref '.*' in '.*' can not be resolved\.$/, /^Problems loading reference /, /^Unable to parse content from /, /^Problem reading content from /,
  /^Draft-03 schemas are not supported\.$/, /^The schema uses meta-schema features /, /^Duplicate anchor declaration: /,
];
const templateHits = new Map(TEMPLATES.map((t) => [t, 0]));
const rejections = new Map();
let customMessages = 0;

function recordCoverage(oracle) {
  if (oracle.error) rejections.set(`${oracle.error.name}: ${oracle.error.message}`, (rejections.get(`${oracle.error.name}: ${oracle.error.message}`) ?? 0) + 1);
  for (const d of oracle.diagnostics ?? []) {
    const t = TEMPLATES.find((x) => x.test(d.message));
    if (t) templateHits.set(t, templateHits.get(t) + 1);
    else customMessages++;
  }
}

// ---- Comparing ----

const groups = {};
const categories = {};
let documents = 0;
let diffs = 0;
const shown = [];

function group(name) {
  return (groups[name] ??= { documents: 0, diffs: 0 });
}
function category(name, reason) {
  return (categories[name] ??= { count: 0, reason, mismatched: 0 });
}

const hasEscapedLoneSurrogate = (text) =>
  /\\u[dD][89abAB][0-9a-fA-F]{2}(?!\\u[dD][c-fC-F][0-9a-fA-F]{2})/.test(text) || /(?<!\\u[dD][89abAB][0-9a-fA-F]{2})\\u[dD][c-fC-F][0-9a-fA-F]{2}/.test(text);
const toUsv = (s) => s.toWellFormed();

function isLowSurrogate(text, offset) {
  const code = text.charCodeAt(offset);
  return code >= 0xdc00 && code <= 0xdfff;
}

function normalizeOracle(json, oracle) {
  if (!oracle.diagnostics) return oracle;
  let midSurrogate = false;
  const diagnostics = oracle.diagnostics.map((d) => {
    let { start, end } = d;
    if (isLowSurrogate(json, start)) {
      start -= 1;
      midSurrogate = true;
    }
    if (isLowSurrogate(json, end)) {
      end -= 1;
      midSurrogate = true;
    }
    return { ...d, start, end };
  });
  if (midSurrogate) {
    category('mid-surrogate-offset', 'upstream steps back one UTF-16 unit into an astral character; a byte offset has to name the whole character').count++;
  }
  return { ...oracle, diagnostics };
}

function compare(groupName, c, oracleRaw, lq) {
  const g = group(groupName);
  g.documents++;
  documents++;
  recordCoverage(oracleRaw);
  const oracle = normalizeOracle(c.json, oracleRaw);
  let a = JSON.stringify(oracle);
  const b = JSON.stringify(lq);
  const escaped = hasEscapedLoneSurrogate(c.json);
  const cat = escaped ? category('escaped-lone-surrogate', 'a string value holds an unpaired surrogate; Rust strings store U+FFFD, so messages are compared with U+FFFD, and stringLength and equality on that value can differ') : null;
  if (cat) {
    cat.count++;
    a = JSON.stringify(JSON.parse(a, (_, v) => (typeof v === 'string' ? toUsv(v) : v)));
  }
  if (a === b) return;
  if (cat) {
    cat.mismatched++;
    return;
  }
  g.diffs++;
  diffs++;
  const text = `[${groupName}] ${c.uri}\n  oracle: ${snippet(oracle)}\n  lq:     ${snippet(lq)}\n  json:   ${snippet(c.json.length > 600 ? c.json.slice(0, 600) : c.json)}\n`;
  fs.appendFileSync(DIFF_FILE, `${text}  full: ${JSON.stringify(c.json)}\n  fullOracle: ${a}\n  fullLq: ${b}\n\n`);
  if (shown.length < SHOW) shown.push(text);
}

async function runBatch(groupName, mode, docs, manifestFile, cases) {
  const usable = [];
  for (const c of cases) {
    if (c.json.isWellFormed()) usable.push(c);
    else category('raw-lone-surrogate', 'the text itself holds an unpaired UTF-16 unit, which a Rust String cannot represent; not compared').count++;
  }
  const oracle = await runOracle(mode, docs, manifestFile, usable);
  const lq = runBin(mode, docs, manifestFile, usable);
  if (lq.length !== usable.length) throw new Error(`binary returned ${lq.length} lines for ${usable.length} cases`);
  usable.forEach((c, i) => compare(c.group ?? groupName, c, oracle[i], lq[i]));
  if (process.env.JSONSCHEMA_PROGRESS) {
    console.error(`[${groupName}] +${usable.length} documents=${documents} diffs=${diffs} heap=${(process.memoryUsage().heapUsed / 1e6) | 0}MB t=${(performance.now() / 1000) | 0}s`);
  }
  return oracle;
}

// ---- Real documents ----

function collectSchemaBodies(file) {
  const src = fs.readFileSync(file, 'utf8');
  if (!src.includes('schema')) return [];
  let ast;
  try {
    ast = toLiquidHtmlAST(src);
  } catch {
    category('liquid-parse-failure', 'the Liquid file does not parse, so it has no schema body to extract').count++;
    return [];
  }
  const found = [];
  const walk = (node) => {
    if (!node || typeof node !== 'object') return;
    if (node.type === 'LiquidRawTag' && node.name === 'schema' && typeof node.body?.value === 'string') found.push(node.body.value);
    for (const child of node.children ?? []) walk(child);
    if (Array.isArray(node.body)) node.body.forEach(walk);
  };
  walk(ast);
  return found;
}

function realDocuments() {
  const docs = [];
  for (const [name, root] of Object.entries(THEMES)) {
    if (!fs.existsSync(root)) {
      category('missing-theme', 'a theme directory is absent').count++;
      console.log(`[jsonschema] theme ${name} missing at ${root}`);
      continue;
    }
    const rootUri = `file://${root.replace(/ /g, '%20')}`;
    for (const dir of ['sections', 'blocks']) {
      const d = path.join(root, dir);
      if (!fs.existsSync(d)) continue;
      for (const file of liquidFiles(d)) {
        for (const json of collectSchemaBodies(file)) {
          docs.push({ uri: `${rootUri}/${path.relative(root, file)}`, json });
        }
      }
    }
    for (const dir of ['templates', 'config', 'locales']) {
      const d = path.join(root, dir);
      if (!fs.existsSync(d)) continue;
      const walk = (p) => {
        for (const e of fs.readdirSync(p, { withFileTypes: true })) {
          const full = path.join(p, e.name);
          if (e.isDirectory()) walk(full);
          else if (e.name.endsWith('.json')) docs.push({ uri: `${rootUri}/${path.relative(root, full)}`, json: fs.readFileSync(full, 'utf8') });
        }
      };
      walk(d);
    }
  }
  return docs;
}

// ---- Mutation ----

const settingTypes = (() => {
  const text = fs.readFileSync(path.join(DOCS, 'setting.json'), 'utf8');
  const found = new Set();
  const walk = (v) => {
    if (Array.isArray(v)) v.forEach(walk);
    else if (v && typeof v === 'object') {
      if (v.properties?.type?.const) found.add(v.properties.type.const);
      for (const x of Object.values(v)) walk(x);
    }
  };
  walk(JSON.parse(text));
  return [...found].sort();
})();

const KNOWN_URIS = [
  'theme/section.json', 'theme/theme_block.json', 'theme/theme_settings.json', 'theme/translations.json',
  'theme/settings.json', 'theme/strict_settings.json', 'theme/setting.json', 'theme/default_setting_values.json',
  'theme/app_block_entry.json', 'theme/theme_block_entry.json', 'theme/targetted_block_entry.json',
  'theme/preset_blocks.json', 'theme/preset.json', 'theme/local_block_entry.json',
].map((u) => `${SCHEMA_ROOT}/${u}`);
const SCHEMA_VALUES = [
  ...KNOWN_URIS,
  `${KNOWN_URIS[0]}#`, `${KNOWN_URIS[3]}?x=1`, KNOWN_URIS[0].replace('https:', 'HTTPS:'),
  'https://example.com/unknown.json', 'http://json-schema.org/draft-07/schema#', 'https://json-schema.org/draft-07/schema',
  'http://json-schema.org/draft-04/schema#', 'https://json-schema.org/draft-04/schema', 'https://json-schema.org/draft/2019-09/schema', 'https://json-schema.org/draft/2020-12/schema', 'http://json-schema.org/draft-06/schema#', 'http://json-schema.org/draft-03/schema#',
  'relative/path.json', './section.json', '../theme/section.json', '/abs/path.json', 'section.json', '', ' ', '#', '?q', '#frag',
  'garbage garbage', 'not a url at all', '//', '//example.com/x.json', '//[bad', '//exa mple/', 'file:///x.json', 'x:y', 'x://',
  'mailto:a@b', '1abc:def', 'héllo.json', '😀.json', 'a\\b.json', '%zz', '%2e%2e/x', 'schema:', ':colon', 'http:', 'https:', 'http://', 'C:\\x.json',
];
const UNKNOWN_STRINGS = ['unknown', '', ' ', 'x', 'nope', 'section', 'template', 'é', '日本語', '😀', 'a'.repeat(300), 't:foo', 'top', '10', '#fff', '10px', '-1px', 'auto', 'https://x.y/z', 'a@b.co', 'not a uri', '\u2028', 'x\ny', 'true', 'null'];
const NUMBERS = [-1, 0, 1, 2, 3, 5, 10, 16, 50, 100, 101, 1000, 1e30, -1e30, 0.1, 0.5, 1.1, 2.5, 0.25, 1e-7, 1.5e-7, 33.3, 1e21, 2147483648, 4.35];
const REQUIRED_KEYS = ['name', 'id', 'type', 'label', 'content', 'min', 'max', 'step', 'options', 'value', 'default', 'settings', 'blocks', 'presets', 'unit', 'theme_name', 'theme_author', 'theme_version', 'theme_documentation_url', 'theme_support_email', 'theme_support_url'];
const JUNK = ['{', '}', '[', ']', ',', ':', '"', "'", '\\', '//', '/*', '*/', 'x', 'true', 'null', '1', '-', '.', '1.', '1e', '01', 'é', '😀', '\u00a0', '\u2028', '\ufeff', '\t', '\r', '\n', '\r\n', '\u0001', '\\u12', '\\q', '\\ud83d', '\\ude00', '\\ud83d\\ude00', '1e400', '-0', '}{', '][', '{}', '[]', '""'];

const clone = (v) => JSON.parse(JSON.stringify(v));

function allRefs(root) {
  const refs = [];
  const walk = (v, parent, key) => {
    if (parent) refs.push({ parent, key, value: v });
    if (Array.isArray(v)) v.forEach((x, i) => walk(x, v, i));
    else if (v && typeof v === 'object') for (const k of Object.keys(v)) walk(v[k], v, k);
  };
  walk(root, null, null);
  return refs;
}

const pick = (r, list) => list[r(list.length)];

function retype(r, value) {
  const options = ['str', 42, 1.5, true, false, null, [], {}, ['x'], { x: 1 }, [1, 2], [[]], [{}], { type: 'text' }];
  return options[r(options.length)] === value ? 'other' : options[r(options.length)];
}

function structuralMutation(root, r) {
  const refs = allRefs(root);
  const objects = refs.filter((x) => x.value && typeof x.value === 'object' && !Array.isArray(x.value));
  if (root && typeof root === 'object' && !Array.isArray(root)) objects.push({ value: root });
  const op = r(13);
  const any = refs.length ? pick(r, refs) : null;
  const set = (ref, v) => {
    if (ref?.parent) ref.parent[ref.key] = v;
  };
  const del = (ref) => {
    if (!ref?.parent) return;
    if (Array.isArray(ref.parent)) ref.parent.splice(ref.key, 1);
    else delete ref.parent[ref.key];
  };
  switch (op) {
    case 0:
      del(any);
      break;
    case 1:
      set(any, retype(r, any?.value));
      break;
    case 2:
      set(any, pick(r, UNKNOWN_STRINGS));
      break;
    case 3: {
      const typed = refs.filter((x) => x.key === 'type');
      const ref = typed.length ? pick(r, typed) : any;
      set(ref, r(4) === 0 ? pick(r, UNKNOWN_STRINGS) : pick(r, settingTypes));
      break;
    }
    case 4: {
      // A duplicate key; the marker becomes the plain key after serialization.
      if (objects.length) {
        const o = pick(r, objects).value;
        const keys = Object.keys(o);
        if (keys.length) o[`${pick(r, keys)}\u0007dup`] = retype(r, null);
      }
      break;
    }
    case 5:
      set(pick(r, objects.length ? objects : refs.length ? refs : [{}]), {});
      break;
    case 6: {
      const strings = refs.filter((x) => typeof x.value === 'string');
      set(strings.length ? pick(r, strings) : any, strings.length && r(2) ? pick(r, strings).value : pick(r, UNKNOWN_STRINGS));
      break;
    }
    case 7: {
      const numbers = refs.filter((x) => typeof x.value === 'number');
      set(numbers.length ? pick(r, numbers) : any, pick(r, NUMBERS));
      break;
    }
    case 8: {
      const named = refs.filter((x) => REQUIRED_KEYS.includes(x.key));
      del(named.length ? pick(r, named) : any);
      break;
    }
    case 9:
      if (objects.length) pick(r, objects).value[pick(r, ['unknown_key', 'x', '//', '$comment', 'é', '😀', '', '__proto__x', '0', '42'])] = retype(r, null);
      break;
    case 10: {
      const composite = refs.filter((x) => x.value && typeof x.value === 'object');
      const ref = composite.length ? pick(r, composite) : null;
      if (ref) set(ref, Array.isArray(ref.value) ? Object.fromEntries(ref.value.map((v, i) => [String(i), v])) : Object.values(ref.value));
      break;
    }
    case 11: {
      // Fields that drive numeric and string keywords of setting.json.
      if (objects.length) {
        const o = pick(r, objects).value;
        o[pick(r, ['min', 'max', 'step', 'default', 'limit', 'max_blocks', 'unit', 'minimum'])] = r(2) ? pick(r, NUMBERS) : pick(r, UNKNOWN_STRINGS);
      }
      break;
    }
    default: {
      const arrays = refs.filter((x) => Array.isArray(x.value) && x.value.length);
      if (arrays.length) {
        const a = pick(r, arrays).value;
        a.push(clone(pick(r, a)));
      }
    }
  }
  return root;
}

function addSchemaKey(root, r) {
  const value = r(12) === 0 ? pick(r, [1, null, true, [], {}]) : pick(r, SCHEMA_VALUES);
  if (root && typeof root === 'object' && !Array.isArray(root)) {
    if (r(3) === 0) {
      root.$schema = value;
      return root;
    }
    return { $schema: value, ...root };
  }
  return root;
}

const serialize = (value, r) => JSON.stringify(value, null, pick(r, [0, 2, 2, 1, '\t'])).replace(/\\u0007dup/g, '');

function textMutation(text, r) {
  const n = text.length;
  switch (r(15)) {
    case 0: {
      const i = r(n + 1);
      return text.slice(0, i) + text.slice(i + 1 + r(3));
    }
    case 1: {
      const i = r(n + 1);
      return text.slice(0, i) + pick(r, JUNK) + text.slice(i);
    }
    case 2:
      return text.slice(0, r(n + 1));
    case 3: {
      const i = r(n + 1);
      return `${text.slice(0, i)}/* comment ${pick(r, ['', 'é', '😀', '*', '"'])} */${text.slice(i)}`;
    }
    case 4: {
      const i = r(n + 1);
      return `${text.slice(0, i)}// line comment\n${text.slice(i)}`;
    }
    case 5:
      return text.replace(/([}\]"\d])(\s*)([}\]])/, '$1,$2$3');
    case 6:
      return text.replace(/\r?\n/g, '\r\n');
    case 7:
      return text.replace(/\r?\n/g, '\r');
    case 8: {
      // Non-ASCII and astral characters inside a string.
      const quotes = [...text.matchAll(/"/g)];
      if (!quotes.length) return text;
      const at = pick(r, quotes).index + 1;
      return text.slice(0, at) + pick(r, ['é', '日本語', '😀', '👩‍💻', 'ß', '\u00a0', '\\u00e9', '\\ud83d\\ude00']) + text.slice(at);
    }
    case 9:
      return `\ufeff${text}`;
    case 10:
      return text + pick(r, ['\n', ' ', '\u00a0', '\u2028', '\ufeff', 'x', '{}', '😀', '\r\n', ',', '\n\n']);
    case 11: {
      // Bad token at the very end, so the error path scans back over whitespace.
      const end = text.trimEnd().length;
      return text.slice(0, r(end + 1)) + pick(r, ['', '😀', 'é', '\u00a0 \n', ' \u2028', '"', '"a', '"😀', '{', '[', ':', ',']) + pick(r, ['', '\n', ' ', '\r\n', '\u00a0']);
    }
    case 12: {
      // Move something onto its own line, so "Colon expected" sees a new line.
      const quotes = [...text.matchAll(/":/g)];
      if (!quotes.length) return text;
      const at = pick(r, quotes).index + 1;
      return `${text.slice(0, at)}${pick(r, ['\n', '\r\n', '\r', ' ', ''])}${text.slice(at + 1)}`;
    }
    case 13: {
      const i = r(n + 1);
      return text.slice(0, i) + pick(r, ['\n', '\r', '\r\n', '\u0001']) + text.slice(i);
    }
    default: {
      // Delete one code unit: this can split a surrogate pair, a named category.
      const i = r(n + 1);
      return text.slice(0, i) + text.slice(i + 1);
    }
  }
}

function mutations(doc, count, seed) {
  const r = rng(hash(doc.uri) ^ hash(doc.json.slice(0, 2000)) ^ seed);
  let parsed;
  const errors = [];
  parsed = jsonc.parse(doc.json, errors, { allowTrailingComma: true });
  const out = [];
  for (let i = 0; i < count; i++) {
    let text = doc.json;
    const kind = r(10);
    if (parsed !== undefined && kind < 7) {
      let value = clone(parsed);
      const rounds = 1 + r(3);
      for (let k = 0; k < rounds; k++) value = structuralMutation(value, r);
      if (kind === 0) value = addSchemaKey(value, r);
      text = serialize(value, r);
      if (r(5) === 0) text = textMutation(text, r);
    } else if (parsed !== undefined && kind === 7) {
      text = serialize(addSchemaKey(clone(parsed), r), r);
    } else {
      const rounds = 1 + r(2);
      for (let k = 0; k < rounds; k++) text = textMutation(text, r);
    }
    out.push({ uri: doc.uri, json: text });
  }
  return out;
}

function settingDocuments(seed) {
  const r = rng(seed);
  const value = () =>
    pick(r, [
      ...NUMBERS, ...UNKNOWN_STRINGS, true, false, null, [], {},
      'px', '%', 'rem', '50', 'center', 'left', ['a'], [{ value: 'a', label: 'b' }], [{ value: 'a', label: 'b' }, { value: 'c', label: 'd' }], { value: 'a' },
      '{{ section.settings.x }}', 't:x.y', '#000', '#00000', 'rgba(0,0,0,1)', 'linear-gradient(red, blue)', 'shopify://x', 'https://a.b', '<p>x</p>', '<h1>x</h1>',
    ]);
  const FIELDS = ['default', 'min', 'max', 'step', 'unit', 'options', 'placeholder', 'info', 'visible_if', 'limit', 'accept', 'metaobject_type', 'presets', 'bad_key', 'label', 'id', 'content', 'markdown'];
  const setting = (type) => {
    const s = {};
    if (r(8)) s.type = type;
    if (r(8)) s.id = pick(r, ['a', 'b_c', 'x-y', '1', 'é', '']);
    if (r(4)) s.label = pick(r, ['Label', 't:x', '', 5]);
    const fields = r(6);
    for (let i = 0; i < fields; i++) s[pick(r, FIELDS)] = value();
    if (type === 'range' && r(2)) {
      Object.assign(s, { min: pick(r, NUMBERS), max: pick(r, NUMBERS), step: pick(r, NUMBERS), default: pick(r, NUMBERS) });
    }
    return s;
  };
  const docs = [];
  for (const type of [...settingTypes, 'unknown', 'text', 'range', 'select']) {
    for (let k = 0; k < 24; k++) {
      const settings = Array.from({ length: 1 + r(3) }, () => setting(r(6) ? type : pick(r, settingTypes)));
      const blockEntries = [{ type: '@app' }, { type: '@theme' }, { type: 'x', name: 'y', settings }, { type: 'x' }, { type: '_local' }, { type: '_local', name: 'y', settings }, { type: 'x', name: 'y', limit: 1, settings }];
      const section = { name: 'x', settings };
      if (r(2)) section.blocks = [pick(r, blockEntries), ...(r(2) ? [pick(r, blockEntries)] : [])];
      if (r(3) === 0) section.presets = [{ name: 'p', settings: { a: value() }, blocks: r(2) ? [{ type: 'x', settings: { a: value() } }] : { a: { type: 'x' } } }];
      if (r(3) === 0) section.default = { settings: { a: value() }, blocks: [{ type: 'x' }] };
      if (r(4) === 0) section[pick(r, ['tag', 'class', 'limit', 'max_blocks', 'locales', 'enabled_on', 'disabled_on', 'templates', 'groups'])] = value();
      docs.push({ uri: 'file:///synth/sections/x.liquid', json: JSON.stringify(section, null, 2) });
      docs.push({ uri: 'file:///synth/blocks/x.liquid', json: JSON.stringify(section, null, 2) });
      docs.push({ uri: 'file:///synth/config/settings_schema.json', json: JSON.stringify([{ name: 'theme_info', theme_name: 'x', theme_version: '1', theme_author: 'a', theme_documentation_url: pick(r, ['https://a.b', 'nope', '']), theme_support_email: pick(r, ['a@b.co', 'nope', '']) }, { name: 'n', settings }], null, 1) });
    }
  }
  const locale = (depth) => {
    if (depth === 0 || r(3) === 0) return pick(r, ['x', '', 5, null, true, [], {}, { one: 'a', other: 'b' }]);
    const o = {};
    const keys = r(5);
    for (let i = 0; i < keys; i++) o[pick(r, ['a', 'b_c', 'x-y', 'one', 'other', 'k1', '1', '$x', 'title_html', 'é', '😀', ''])] = locale(depth - 1);
    return o;
  };
  for (let i = 0; i < 600; i++) {
    docs.push({ uri: pick(r, ['file:///synth/locales/en.default.json', 'file:///synth/locales/fr.json', 'file:///synth/locales/en.default.schema.json']), json: JSON.stringify(locale(4), null, 2) });
  }
  return docs;
}

// ---- Hand-written traps ----

function trapDocuments() {
  const uri = 'file:///t/sections/a.liquid';
  const json = [
    '', ' ', '\n', '\r\n', '{', '}', '[', ']', '{}', '[]', 'null', 'true', 'false', '0', '-', '-1', '1.', '1.e5', '1e', '1e400', '-1e400', '"', '""', '"a', '"\\', '"\\u', '"\\u12"', '"\\q"',
    '/*', '/* x', '//', '// x', '/**/', '{"a":1,}', '[1,]', '{,}', '[,]', '{"a"}', '{"a":}', '{"a":,}', '{"a" 1}', '{a:1}', "{'a':1}", '{1:2}', '{"a":1 "b":2}', '[1 2]', '{"a":1}}', '[1]]', '{} {}', '{}x',
    '{"a":1,"a":2}', '{"a":1,"a":2,"a":3}', '{"//":1,"//":2}', '{"name":"x","name":"y"}', '{"name":"x","name"}',
    '{"name"\n"settings":[]}', '{"name"\r\n"settings":[]}', '{"name"\r"settings":[]}', '{"name" "settings":[]}', '{"name"/*\n*/"settings":[]}',
    '\ufeff{}', '\ufeff', '{}\ufeff', '{\ufeff}', '😀', '{😀}', '{"a":😀', '{"a":😀 ', '{"a":😀\n', '[😀', '{"😀":1,"😀":2}', '{"😀"', '{"é"', '"😀', '{"name":"😀","x":1}', 'é', '{é', '\u00a0', '{\u00a0', '{"a":\u00a0', '{"a":1\u2028', '{"a"\u2028:1}',
    '{"name":"x"\r\n', '{"name":"x",\r\n', '{\r\n"name":5\r\n}\r\n', '{"name":5}\r\n', '[\r\n', '\r\n{', '{\r\n\r\n', '{\n\r', '{"$schema":"x"\r\n',
    '{"$schema":"https://example.com/x.json"}', '{"x":1,"$schema":"https://example.com/x.json"}', '{"$schema":5}', '{"$schema":5,"$schema":"x:y"}', '{"$schema"}', '{"$schema":}', '{"$schema":"https://example.com/x.json"',
    `{"$schema":"${KNOWN_URIS[0]}"}`, `{"$schema":"${KNOWN_URIS[0]}","name":5}`, `{"name":5,"$schema":"${KNOWN_URIS[3]}"}`, `{"$schema":"${KNOWN_URIS[6]}","type":"range"}`, `{"$schema":"${KNOWN_URIS[6]}"}`,
    '{"name":"\\ud83d"}', '{"name":"\\ude00"}', '{"\\ud83d":1}', '{"name":"\\ud83d\\ude00"}',
    '{"settings":[{"type":"range","id":"a","label":"b","min":0,"max":1,"step":0.1,"default":0.3}]}',
    '{"settings":[{"type":"range","id":"a","label":"b","min":0,"max":1.1,"step":0.1,"default":1e-7}]}',
    '{"name":"x","settings":[{"type":"text"},{"type":"text"}]}',
  ];
  const deep = (n, open, close) => open.repeat(n) + close.repeat(n);
  // Between about 500 and 5000 levels, whether upstream overflows V8's stack depends on
  // what the JIT has optimized so far, so only clear-cut depths are compared.
  for (const n of [10, 50, 100, 500]) {
    json.push(deep(n, '[', ']'), deep(n, '{"a":', '}').replace(/:\}/, ':1}'), `{"settings":${deep(n, '[', ']')}}`);
  }
  for (const n of [100000]) json.push('['.repeat(n), deep(n, '[', ']'));
  const out = [];
  for (const j of json) {
    for (const u of [uri, 'file:///t/blocks/a.liquid', 'file:///t/config/settings_schema.json', 'file:///t/locales/en.json', 'file:///t/templates/x.json', 'file:///a%20b/locales/en.json?x=1#y', 'not a uri', '', 'file:', 'x:y']) {
      out.push({ uri: u, json: j });
    }
  }
  return out;
}

// ---- Synthetic schemas ----

function synthBatch(seed) {
  const r = rng(seed);
  const NAMES = ['a', 'b', 'c', 'kind', 'x_y', 'é', 'n', 'list', '1', '2'];
  const STR = ['a', 'b', 'c', 'x', 'kind1', 'kind2', 'é', '😀', 'abc', '', 'A', 'hello world', '2024-01-02', '12:00:00Z', '2024-01-02T12:00:00Z', 'a@b.co', 'foo.example', '1.2.3.4', '::1', '#fff', 'http://a.b', '/rel', 'a:b'];
  const NUM = [0, 1, 2, 3, -1, 0.1, 0.2, 0.3, 0.5, 1.5, 2.5, 10, 100, 1e21, 1e-7, 3.3, 0.07, 1.1];
  const MESSAGES = ['custom message', 'other {0} message', 'é message', '$& dollars'];
  const FILES = 6;
  
  const scalar = () => pick(r, [pick(r, STR), pick(r, STR), pick(r, NUM), pick(r, NUM), true, false, null]);
  const anyValue = (d = 2) => {
    const k = r(6);
    if (d <= 0 || k < 3) return scalar();
    if (k === 3) return Array.from({ length: r(3) }, () => anyValue(d - 1));
    const o = {};
    for (let i = r(3); i > 0; i--) o[pick(r, NAMES)] = anyValue(d - 1);
    return o;
  };

  const schema = (d) => {
    const pickWeird = r(90) === 0;
    if (pickWeird) return pick(r, [true, false, true, false, null, 5, 'str', [], '', 0]);
    const s = {};
    const n = 1 + r(d > 0 ? 4 : 2);
    for (let i = 0; i < n; i++) keyword(s, d);
    return s;
  };
  const subSchemas = (d, count) => Array.from({ length: count }, () => schema(d - 1));

  const keyword = (s, d) => {
    const k = r(d > 0 ? 60 : 26);
    switch (k) {
      case 0: case 1: case 2:
        s.type = pick(r, ['string', 'number', 'integer', 'boolean', 'null', 'object', 'array', 'unknown']);
        break;
      case 3:
        s.type = r(10) === 0 ? pick(r, [5, true, { '0': 'x' }, { a: 1 }, {}, [null, 1, ['a', 'b'], {}]]) : [pick(r, ['string', 'integer']), pick(r, ['object', 'null', 'array', 'x'])];
        break;
      case 4: case 5:
        s.enum = Array.from({ length: r(4) }, () => (r(8) === 0 ? anyValue(2) : scalar()));
        break;
      case 6: case 7:
        s.const = r(8) === 0 ? anyValue(2) : scalar();
        break;
      case 8: s.minLength = pick(r, [0, 1, 2, 3]); break;
      case 9: s.maxLength = pick(r, [0, 1, 2, 3]); break;
      case 10:
        s.pattern = pick(r, ['^a', 'b$', '^[a-c]+$', '(?i)^A', '.*', '^.$', '\\w+', '\\d', '(', '[', 'a{', '\\-', '\\p{L}', '^\\p{Lu}', '(?<n>a)\\k<n>', '.{2}', '^\\s*$', '(?=a)', 'a|b', '\\u{1F600}', '[😀]']);
        break;
      case 11:
        s.format = pick(r, ['uri', 'uri-reference', 'color-hex', 'date-time', 'date', 'time', 'email', 'hostname', 'ipv4', 'ipv6', 'unknown-format']);
        break;
      case 12: s.minimum = pick(r, NUM); break;
      case 13: s.maximum = pick(r, NUM); break;
      case 14: s.exclusiveMinimum = pick(r, [...NUM, true, false]); break;
      case 15: s.exclusiveMaximum = pick(r, [...NUM, true, false]); break;
      case 16: s.multipleOf = pick(r, [1, 2, 3, 0.1, 0.01, 0.5, 0.3, 1.5, 1e-7, 0, 1e21, 2.5]); break;
      case 17: s.required = Array.from({ length: 1 + r(2) }, () => (r(30) === 0 ? pick(r, [1, null, true]) : pick(r, NAMES))); break;
      case 18: s.errorMessage = pick(r, MESSAGES); break;
      case 19: s.patternErrorMessage = pick(r, MESSAGES); break;
      case 20: s.deprecationMessage = pick(r, MESSAGES); break;
      case 21: s.deprecated = true; break;
      case 22: s.minProperties = pick(r, [0, 1, 2]); break;
      case 23: s.maxProperties = pick(r, [0, 1, 2]); break;
      case 24: s.minItems = pick(r, [0, 1, 2]); s.maxItems = pick(r, [0, 1, 2]); break;
      case 25: s.uniqueItems = pick(r, [true, true, false, 1]); break;
      case 26: case 27: case 28: case 29: case 30: {
        const count = 1 + r(3);
        s.properties = s.properties ?? {};
        for (let i = 0; i < count; i++) s.properties[pick(r, NAMES)] = schema(d - 1);
        break;
      }
      case 31: s.additionalProperties = pick(r, [true, false, schema(d - 1)]); break;
      case 32: s.patternProperties = { [pick(r, ['^a', 'b$', '^x', '.', '(', '(?i)^KIND', '^\\d+$'])]: schema(d - 1) }; break;
      case 33: s.unevaluatedProperties = pick(r, [false, true, schema(d - 1)]); break;
      case 34: case 35: s.allOf = subSchemas(d, 1 + r(3)); break;
      case 36: case 37: s.anyOf = subSchemas(d, 1 + r(4)); break;
      case 38: case 39: s.oneOf = subSchemas(d, 1 + r(4)); break;
      case 40: case 41: {
        // Discriminated alternatives, where the optimization applies.
        const key = pick(r, ['kind', 'a']);
        const alts = ['kind1', 'kind2', 'x'].slice(0, 2 + r(2)).map((v) => {
          const alt = schema(d - 1);
          const o = alt && typeof alt === 'object' && !Array.isArray(alt) ? alt : {};
          o.properties = { ...(o.properties ?? {}), [key]: { const: r(10) === 0 ? pick(r, [1, null, ['x'], 'a']) : v }, ...(r(2) ? { b: schema(d - 2) } : {}) };
          return o;
        });
        s[pick(r, ['oneOf', 'anyOf'])] = alts;
        break;
      }
      case 42: {
        // Alternatives over enums, for message merging.
        s[pick(r, ['oneOf', 'anyOf'])] = Array.from({ length: 2 + r(2) }, () => ({ enum: Array.from({ length: 1 + r(3) }, () => pick(r, STR.slice(0, 6))) }));
        break;
      }
      case 43: {
        const alts = Array.from({ length: 2 + r(2) }, () => ({ prefixItems: [{ const: pick(r, ['a', 'b']) }, schema(d - 1)] }));
        s[pick(r, ['oneOf', 'anyOf'])] = r(2) ? alts : alts.map((x) => ({ items: x.prefixItems }));
        break;
      }
      case 44: s.not = schema(d - 1); break;
      case 45: case 46: s.if = schema(d - 1); s.then = schema(d - 1); if (r(2)) s.else = schema(d - 1); break;
      case 47: s.items = pick(r, [schema(d - 1), subSchemas(d, 1 + r(3)), true, false]); break;
      case 48: s.prefixItems = r(30) === 0 ? pick(r, [null, 'ab', 5]) : subSchemas(d, 1 + r(2)); break;
      case 49: s.additionalItems = pick(r, [true, false, schema(d - 1)]); break;
      case 50: s.contains = schema(d - 1); if (r(2)) s.minContains = pick(r, [0, 1, 2]); if (r(2)) s.maxContains = pick(r, [0, 1, 2]); break;
      case 51: s.unevaluatedItems = pick(r, [false, true, schema(d - 1)]); break;
      case 52: s.dependencies = { [pick(r, NAMES)]: r(2) ? [pick(r, NAMES), pick(r, NAMES)] : schema(d - 1) }; break;
      case 53: s.dependentRequired = { [pick(r, NAMES)]: [pick(r, NAMES)] }; break;
      case 54: s.dependentSchemas = { [pick(r, NAMES)]: schema(d - 1) }; break;
      case 55: s.propertyNames = schema(d - 1); break;
      case 56: case 57: case 58:
        s.$ref = reference();
        break;
      default:
        if (r(4) === 0) s[pick(r, ['$id', 'id'])] = `#${pick(r, ['an1', 'an2'])}`;
        else if (r(3) === 0) s.$anchor = pick(r, ['an1', 'an2']);
        else if (r(10) === 0) s[pick(r, ['$recursiveRef', '$dynamicRef', '$recursiveAnchor', '$dynamicAnchor'])] = pick(r, ['#', true]);
        else if (r(10) === 0) s[pick(r, ['allowComments', 'allowTrailingCommas', 'allowsTrailingCommas'])] = pick(r, [true, false]);
        else s.definitions = { ...(s.definitions ?? {}), [pick(r, ['d1', 'd2', 'd3'])]: schema(d - 1) };
    }
  };

  let badRefs = false;
  let currentFile = 0;
  // External references only point forward. A cycle of them through unresolved
  // sections never settles upstream: it grows promises until the heap is gone.
  const laterFile = () => (currentFile + 1 < FILES ? currentFile + 1 + r(FILES - currentFile - 1) : null);
  const reference = () => {
    const good = r(30);
    const later = laterFile();
    if (!badRefs || good < 20) {
      if (good < 16) return `#/definitions/${pick(r, ['d1', 'd2', 'd3'])}`;
      if (good < 24 && later !== null) return `./f${later}.json${pick(r, ['', '#', '#/definitions/d1', '#/definitions/d2', '#/definitions/d3'])}`;
      if (good < 26) return `#${pick(r, ['an1', 'an2'])}`;
      if (good < 28 || later === null) return '#';
      return `${SCHEMA_ROOT}/synth/f${later}.json`;
    }
    const k = r(12);
    if (k < 3 && later !== null) return `./f${later}.json${pick(r, ['#/properties/a', '#an1', '#/definitions/nope'])}`;
    if (k === 3) return `#${pick(r, ['nope'])}`;
    if (k === 4) return `./missing.json${pick(r, ['', '#/x', '#an'])}`;
    if (k < 8) return pick(r, ['#/properties/a/type', '#/%zz', '#/definitions/d1/properties', '#/$schema', '#/constructor', '#/enum/0', '#/definitions/d~11', '#/required/length']);
    if (k < 10) return pick(r, [5, true, [], {}, null, 'relative', '//', 'x:y#a', 'http://[bad/x', '//[bad/x']);
    return `#/definitions/d${1 + r(3)}/properties/${pick(r, NAMES)}`;
  };

  const files = [];
  const roots = [];
  for (let i = 0; i < FILES; i++) {
    badRefs = r(6) === 0;
    currentFile = i;
    const s = schema(3);
    const root = s && typeof s === 'object' && !Array.isArray(s) ? s : { oneOf: [s] };
    if (r(3)) root.$schema = pick(r, ['http://json-schema.org/draft-07/schema#', 'http://json-schema.org/draft-04/schema#', 'https://json-schema.org/draft/2019-09/schema', 'https://json-schema.org/draft/2020-12/schema', 'http://json-schema.org/draft-06/schema', 'https://example.com/other', ...(r(40) === 0 ? ['http://json-schema.org/draft-03/schema#', 5] : [])]);
    root.definitions = root.definitions ?? {};
    for (const d of ['d1', 'd2', 'd3']) if (!root.definitions[d] || r(2)) root.definitions[d] = schema(2);
    // `$ref`s into a section whose own external `$ref` is resolved or still pending,
    // depending on which one the traversal reaches first.
    const later = laterFile();
    if (later !== null && r(3) === 0) {
      root.definitions.d1 = { $ref: `./f${later}.json${pick(r, ['', '#/definitions/d1', '#/definitions/d2'])}`, ...(r(2) ? { description: 'sibling' } : {}) };
      const holder = pick(r, ['additionalProperties', 'not', 'contains', 'propertyNames', 'then', 'properties']);
      const link = { $ref: '#/definitions/d1', ...(r(2) ? { minProperties: 1 } : {}) };
      root[holder] = holder === 'properties' ? { ...(typeof root.properties === 'object' ? root.properties : {}), a: link } : link;
    }
    roots.push(root);
    let text = JSON.stringify(root, null, 1);
    if (r(60) === 0) text = pick(r, [`\ufeff${text}`, `${text},`, text.slice(0, text.length - 1), '5', 'null', '"str"', '[]', 'true', 'false', '0', '', '   ', 'nonsense']);
    files.push(text);
  }

  const GOOD_FORMAT = { uri: ['http://a.b', 'a:b', 'nope', '', '/rel'], 'uri-reference': ['/rel', 'x', ''], 'color-hex': ['#fff', '#ffff', '#ffffff', '#ffffffff', '#ff', '#ggg', ''], 'date-time': ['2024-01-02T12:00:00Z', '2024-01-02t12:00:60.5+01:00', '2024-13-02T12:00:00Z', 'x', ''], date: ['2024-01-02', '2024-1-2', ''], time: ['12:00:00Z', '25:00:00Z', ''], email: ['a@b.co', 'a.b@c.de', '"x y"@a.io', 'a@[1.2.3.4]', 'nope', 'a@b', ''], hostname: ['foo.example', 'foo.', '-x', 'a_b', `${'a'.repeat(64)}.x`, ''], ipv4: ['1.2.3.4', '256.1.1.1', '1.2.3', ''], ipv6: ['::1', '1:2:3:4:5:6:7:8', '::ffff:1.2.3.4', 'x', ''], 'unknown-format': ['x'] };

  const resolvePointer = (root, ref) => {
    let cur = root;
    for (const part of ref.split('/').slice(1)) cur = cur?.[part];
    return cur;
  };

  // A value that mostly fits the schema, so validation gets deep before failing.
  const fromSchema = (s, root, depth) => {
    if (depth <= 0 || !s || typeof s !== 'object' || Array.isArray(s) || r(14) === 0) return anyValue(1);
    if (typeof s.$ref === 'string') {
      if (s.$ref.startsWith('#/')) return fromSchema(resolvePointer(root, s.$ref.slice(1)), root, depth - 1);
      if (s.$ref === '#') return fromSchema(root, root, depth - 1);
      const m = /f(\d+)\.json(?:#(\/.*))?$/.exec(s.$ref);
      if (m && roots[m[1]]) {
        const other = roots[m[1]];
        return fromSchema(m[2] ? resolvePointer(other, m[2]) : other, other, depth - 1);
      }
      return anyValue(1);
    }
    for (const key of ['oneOf', 'anyOf']) {
      if (Array.isArray(s[key]) && s[key].length && r(5)) return fromSchema(pick(r, s[key]), root, depth - 1);
    }
    if (Array.isArray(s.allOf) && s.allOf.length && r(3)) {
      const parts = s.allOf.map((x) => fromSchema(x, root, depth - 1));
      if (parts.every((p) => p && typeof p === 'object' && !Array.isArray(p))) return Object.assign({}, ...parts);
      return parts[0];
    }
    if (s.if !== undefined && r(2)) return fromSchema(r(2) ? s.if : pick(r, [s.then, s.else]), root, depth - 1);
    if (s.const !== undefined && r(6)) return s.const;
    if (Array.isArray(s.enum) && s.enum.length && r(6)) return pick(r, s.enum);
    const type = Array.isArray(s.type) ? pick(r, s.type) : s.type;
    if (s.properties || s.required || s.additionalProperties !== undefined || s.patternProperties || s.dependencies || s.dependentRequired || s.dependentSchemas || s.unevaluatedProperties !== undefined || s.propertyNames !== undefined || type === 'object') {
      const o = {};
      for (const [k, v] of Object.entries(typeof s.properties === 'object' && s.properties ? s.properties : {})) if (r(5)) o[k] = fromSchema(v, root, depth - 1);
      for (const k of Array.isArray(s.required) ? s.required : []) if (r(6) && !(k in o)) o[k] = anyValue(1);
      for (const k of Object.keys(s.dependencies ?? {})) if (r(2)) o[k] = anyValue(1);
      for (const k of Object.keys(s.dependentRequired ?? {})) if (r(2)) o[k] = anyValue(1);
      for (const k of Object.keys(s.dependentSchemas ?? {})) if (r(2)) o[k] = anyValue(1);
      if (r(3) === 0) o[pick(r, NAMES)] = fromSchema(s.additionalProperties ?? s.unevaluatedProperties, root, depth - 1);
      return o;
    }
    if (s.items !== undefined || s.prefixItems !== undefined || s.contains !== undefined || s.minItems !== undefined || s.uniqueItems !== undefined || s.unevaluatedItems !== undefined || type === 'array') {
      const list = Array.isArray(s.items) ? s.items : Array.isArray(s.prefixItems) ? s.prefixItems : [];
      const a = list.map((x) => fromSchema(x, root, depth - 1));
      for (let i = r(4); i > 0; i--) a.push(s.contains !== undefined && r(2) ? fromSchema(s.contains, root, depth - 1) : !Array.isArray(s.items) && s.items !== undefined ? fromSchema(s.items, root, depth - 1) : anyValue(1));
      if (r(4) === 0 && a.length) a.push(a[0]);
      return a;
    }
    if (typeof s.format === 'string' && GOOD_FORMAT[s.format]) return pick(r, GOOD_FORMAT[s.format]);
    if (s.minLength !== undefined || s.maxLength !== undefined || s.pattern !== undefined || type === 'string') return pick(r, ['', 'a', 'ab', 'abc', 'abcd', '😀', '😀😀', '😀😀😀', 'é', 'A', 'Ab', 'b', '1', ' ', 'aa', 'ÀB']);
    if (s.multipleOf !== undefined || s.minimum !== undefined || s.maximum !== undefined || s.exclusiveMinimum !== undefined || s.exclusiveMaximum !== undefined || type === 'number' || type === 'integer') {
      const near = [s.minimum, s.maximum, s.exclusiveMinimum, s.exclusiveMaximum, s.multipleOf].filter((x) => typeof x === 'number');
      const base = near.length ? pick(r, near) : pick(r, NUM);
      return pick(r, [base, base * 2, base * 3, base + 1, base - 1, base + 0.1, base * 7, 0.3, 0.6, 0.07, 1e21, 1e-7, 4.35, 1.1, -0.3, 1.0, 33]);
    }
    if (type === 'boolean') return r(2) === 0;
    if (type === 'null') return null;
    return anyValue(2);
  };

  const manifest = { schemas: [] };
  const cases = [];
  for (let i = 0; i < FILES; i++) {
    const entry = { uri: `synth/f${i}.json` };
    if (r(5)) entry.fileMatch = [`s${i}/*.json`, ...(r(10) === 0 ? ['shared/*.json'] : [])];
    if (r(30) === 0) entry.fileMatch = ['{a,b}/*.json', '!x/*.json', '/c/**/d?.json', 'e[0-9].json', '[bad/*.json'];
    manifest.schemas.push(entry);
    for (let k = 0; k < 16; k++) {
      let json = JSON.stringify(fromSchema(roots[i], roots[i], 7), null, r(2) ? 1 : 0) ?? 'null';
      if (r(8) === 0) json = textMutation(json, r);
      cases.push({ uri: pick(r, [`file:///w/s${i}/doc${k % 3}.json`, `file:///w/s${i}/doc.json`, `file:///w/shared/d${k % 2}.json`, 'file:///w/a/x.json', 'file:///w/c/k/d1.json']), json });
    }
  }
  // File order is the common case; shuffle sometimes to vary which
  // entry point resolves first.
  if (r(2)) cases.sort(() => r(3) - 1);
  return { files, manifest, cases };
}

// ---- Hand-written schemas ----

function handwrittenScenarios() {
  const s = (files, docs, match) => ({ files, docs, match: match ?? { f0: ['t/*.json'] } });
  const uri = 'file:///w/t/doc.json';
  const d = (...values) => values.map((v) => (typeof v === 'string' ? v : JSON.stringify(v)));
  return [
    // Enum messages merged across alternatives, and the best-match choice.
    s({ f0: { anyOf: [{ enum: ['a', 'b'] }, { enum: ['c'] }, { const: 'd' }, { type: 'number', enum: [1] }] } }, d('"x"', '"a"', '5', '1', 'null', '{}')),
    s({ f0: { oneOf: [{ enum: ['a', 'b'] }, { enum: ['b', 'c'] }] } }, d('"b"', '"a"', '"z"')),
    s({ f0: { oneOf: [{ enum: ['a'], errorMessage: 'one' }, { enum: ['b'], errorMessage: 'two' }] } }, d('"z"', '"a"')),
    s({ f0: { properties: { p: { anyOf: [{ enum: ['a'] }, { const: 'b' }] } }, required: ['p', 'q'] } }, d({ p: 'z' }, { p: 'a' }, {}, { q: 1 })),
    s({ f0: { oneOf: [{ properties: { a: { const: 1 }, b: { const: 2 } } }, { properties: { a: { const: 1 }, c: { const: 3 } } }, { properties: { a: { enum: [1, 2] } } }] } }, d({ a: 1 }, { a: 1, b: 2 }, { a: 1, b: 9 }, { a: 2, c: 3 }, { b: 2, c: 3 }, { a: 9 })),
    s({ f0: { anyOf: [{ required: ['a'], properties: { a: { type: 'string' }, b: { type: 'string' } } }, { required: ['b'], properties: { b: { type: 'number' } } }] } }, d({ a: 'x' }, { a: 1 }, { b: 1 }, { b: 'x' }, { a: 'x', b: 'y' }, {}, { a: 1, b: 1 })),
    // The discriminator: found, value without alternative, non-string value, duplicates.
    s({ f0: { oneOf: [{ properties: { kind: { const: 'a' }, x: { type: 'string' } }, required: ['x'] }, { properties: { kind: { const: 'b' }, y: { type: 'number' } }, required: ['y'] }, { properties: { kind: { const: 'b' }, z: { type: 'number' } }, required: ['z'] }] } }, d({ kind: 'a' }, { kind: 'b' }, { kind: 'c' }, { kind: 1 }, {}, { x: 1 }, '{"kind":"a","kind":"b"}', '{"kind":5,"kind":"a"}', '{"kind"}', { kind: 'b', y: 1, z: 1 })),
    s({ f0: { anyOf: [{ properties: { kind: { const: 'a' } } }, { properties: { other: { const: 'b' } } }] } }, d({ kind: 'a' }, { kind: 'z' }, { other: 'z' }, { kind: 'z', other: 'z' })),
    s({ f0: { anyOf: [{ properties: { kind: { const: 'a' } } }, { type: 'object' }] } }, d({ kind: 'z' }, { kind: 'a' })),
    s({ f0: { oneOf: [{ properties: { kind: { const: 'a' } } }, { properties: { kind: { const: 1 } } }, { properties: { kind: true, other: { const: 'x' } } }] } }, d({ kind: 'a' }, { kind: 1 }, { other: 'x' })),
    s({ f0: { oneOf: [{ prefixItems: [{ const: 'a' }, { type: 'number' }] }, { prefixItems: [{ const: 'b' }, { type: 'string' }] }] } }, d(['a', 1], ['a', 'x'], ['b', 'x'], ['c'], [1], [])),
    s({ f0: { $schema: 'http://json-schema.org/draft-07/schema#', oneOf: [{ items: [{ const: 'a' }, { type: 'number' }], additionalItems: false }, { items: [{ const: 'b' }] }] } }, d(['a', 1], ['a', 1, 2], ['b', 2, 3], ['c'])),
    s({ f0: { oneOf: [null, { type: 'object' }] } }, d({ a: 1 }, [1], 1)),
    s({ f0: { oneOf: [{ properties: { a: null } }, { properties: { a: { const: 1 } } }] } }, d({ a: 1 }, 1)),
    s({ f0: { oneOf: [{ prefixItems: 5 }, { items: [] }] } }, d([1], 1)),
    // if/then/else, with processed properties feeding unevaluatedProperties.
    s({ f0: { if: { properties: { a: { const: 1 } }, required: ['a'] }, then: { required: ['b'] }, else: { required: ['c'] } } }, d({ a: 1 }, { a: 2 }, {}, { a: 1, b: 1 }, { c: 1 })),
    s({ f0: { if: true, then: false } }, d('1', '{}')),
    s({ f0: { if: false, else: { type: 'string' } } }, d('1', '"a"')),
    s({ f0: { if: {}, then: null } }, d('1')),
    s({ f0: { allOf: [{ properties: { a: true } }], if: { properties: { b: true } }, anyOf: [{ properties: { c: true } }, { properties: { d: true } }], unevaluatedProperties: false } }, d({ a: 1, b: 1, c: 1, d: 1, e: 1 }, { e: 1 })),
    s({ f0: { oneOf: [{ properties: { a: true } }, { properties: { b: true } }], unevaluatedProperties: { type: 'string' } } }, d({ a: 1 }, { a: 1, b: 2 }, { c: 1 }, { c: 'x' })),
    s({ f0: { prefixItems: [true], contains: { type: 'string' }, unevaluatedItems: false } }, d([1, 'a'], [1, 2], [1, 'a', 2])),
    // Draft dependent array keywords.
    ...['http://json-schema.org/draft-04/schema#', 'https://json-schema.org/draft/2019-09/schema', 'https://json-schema.org/draft/2020-12/schema', undefined, 'https://example.com/x', 'http://json-schema.org/draft-07/schema'].flatMap(($schema) => [
      s({ f0: { $schema, items: [{ type: 'string' }, { type: 'number' }], additionalItems: { type: 'null' } } }, d(['a', 1, null], ['a', 1, 2], [1], [])),
      s({ f0: { $schema, items: [{ type: 'string' }], additionalItems: false } }, d(['a', 1], ['a'])),
      s({ f0: { $schema, prefixItems: [{ type: 'string' }], items: { type: 'number' } } }, d(['a', 1], ['a', 'b'], [1])),
      s({ f0: { $schema, prefixItems: [{ type: 'string' }], items: false } }, d(['a', 1], ['a'])),
      s({ f0: { $schema, contains: { type: 'string' }, unevaluatedItems: { type: 'number' } } }, d(['a', 1], ['a', 'b'], [null], [])),
    ]),
    // Messages and keywords.
    s({ f0: { type: 'string', pattern: '^a', minLength: 2, maxLength: 1, format: 'email', errorMessage: 'E' } }, d('"b"', '1', '"ab"', '""')),
    s({ f0: { pattern: '^a', patternErrorMessage: 'P', format: 'uri', errorMessage: 'E', type: 'object', enum: [1], const: 1, not: {}, contains: {}, required: ['a'] } }, d('"b"', '{}', '1', '[]')),
    s({ f0: { properties: { a: false }, patternProperties: { '^b': false }, additionalProperties: false, errorMessage: 'E' } }, d({ a: 1, b: 1, c: 1 })),
    s({ f0: { properties: { a: { deprecationMessage: 'old' }, b: { deprecated: true }, c: { deprecated: false }, d: { deprecationMessage: '', deprecated: true } }, deprecationMessage: 'root' } }, d({ a: 1, b: 1, c: 1, d: 1 }, '1', [{}])),
    s({ f0: { propertyNames: { pattern: '^a', minLength: 2, deprecationMessage: 'key', enum: ['ab'] } } }, d({ ab: 1, b: 2 }, '{"b"}', '{b:1}', {})),
    s({ f0: { dependencies: { a: ['b', 'c'], d: { required: ['e'] }, f: 5, g: null, h: true, i: false }, dependentRequired: { j: ['k'], l: 5 }, dependentSchemas: { m: { required: ['n'] }, o: true, p: [] } } }, d({ a: 1 }, { a: 1, b: 1, c: 1 }, { d: 1 }, { f: 1 }, { g: 1 }, { h: 1 }, { i: 1 }, { j: 1 }, { l: 1 }, { m: 1 }, { o: 1 }, { p: 1 }, '{"a"}')),
    ...[1, 2, 3, 0.1, 0.01, 0.5, 0.3, 1.5, 1e-7, 0, 1e21, 2.5, 1.1, 4.35, 1e-3, 0.07, 100, 1.23456789].map((multipleOf) => s({ f0: { multipleOf } }, d('0', '1', '0.3', '0.6', '0.9', '1.1', '4.35', '435', '1e21', '1e22', '1e-7', '3e-7', '1.5', '3', '-0.3', '-3', '0.07', '0.14', '0.21', '19.99', '1e400', '-0', '1.', '123456789.12345678'))),
    s({ f0: { minimum: 1, maximum: 2, exclusiveMinimum: true, exclusiveMaximum: true } }, d('1', '2', '1.5', '0', '3')),
    s({ f0: { minimum: 1, maximum: 2, exclusiveMinimum: false, exclusiveMaximum: false } }, d('1', '2', '0', '3')),
    s({ f0: { minimum: 1, exclusiveMinimum: 2, maximum: 5, exclusiveMaximum: 4 } }, d('1', '2', '3', '4', '5', '6', '1e400', '-1e400')),
    s({ f0: { exclusiveMinimum: true, exclusiveMaximum: true, minimum: '1', maximum: null } }, d('1')),
    s({ f0: { minLength: 2, maxLength: 2 } }, d('"😀"', '"😀😀"', '"😀😀😀"', '"a"', '"\\ud83d\\ude00"', '"é"', '"éé"', '"👩‍💻"')),
    s({ f0: { uniqueItems: true } }, d([1, 1], [1, 2], [1, 1.0], ['a', 'a'], [[1], [1]], [{ a: 1 }, { a: 1 }], [{ a: 1, b: 2 }, { b: 2, a: 1 }], '[{"a":1,"a":2},{"a":2}]', '[{"a":1,"a":2},{"a":1}]', [null, null], [true, false], [true, true], '[{"a"},{"a"}]', '[{"a"},{}]', '[{"a":1,"a"},{"a":1}]', [[], []], [{}, {}], [[], {}], '[1e400,1e400]', '[-0,0]')),
    s({ f0: { enum: [{ a: 1, b: [1, { c: 2 }] }, [1, 2], null, 1, 'x', true, {}, [], 1e400, -0] } }, d({ a: 1, b: [1, { c: 2 }] }, { b: [1, { c: 2 }], a: 1 }, { a: 1, b: [1, { c: 3 }] }, [1, 2], [2, 1], null, 1, 'x', true, false, {}, [], '1e400', '0', '-0', '{"a":1,"b":[1,{"c":2}],"a":1}', '{"":1}', 'é')),
    s({ f0: { const: { a: [1, 'é', '😀', '\u0001\n"'] } } }, d({ a: 1 }, { a: [1, 'é', '😀', '\u0001\n"'] })),
    s({ f0: { const: null } }, d('null', '1')),
    s({ f0: { const: 0 } }, d('0', '-0', '0.0', '1')),
    s({ f0: { const: false } }, d('false', 'true', '0', '""')),
    ...['color-hex', 'date-time', 'date', 'time', 'email', 'hostname', 'ipv4', 'ipv6', 'uri', 'uri-reference', 'regex', 5, null, ''].map((format) => s({ f0: { format } }, d('""', '"a"', '"#fff"', '"#FFFF"', '"#ffffff"', '"#ffffffff"', '"#ff"', '"#gggggg"', '"2024-01-02T03:04:05Z"', '"2024-01-02T03:04:05.123+01:00"', '"2024-01-02t03:04:05z"', '"2024-13-02T03:04:05Z"', '"2024-01-02"', '"2024-02-31"', '"03:04:05Z"', '"24:00:00Z"', '"a@b.co"', '"a.b+c@d-e.fg"', '"\\"a b\\"@c.de"', '"a@[1.2.3.4]"', '"a@b"', '"a b@c.de"', '"foo.example"', '"Foo.Example."', '"-a"', '"a-"', '"a..b"', '"1.2.3.4"', '"256.0.0.1"', '"01.2.3.4"', '"::"', '"::1"', '"1:2:3:4:5:6:7:8"', '"1::8"', '"::ffff:1.2.3.4"', '"1:2:3:4:5:6:7:8:9"', '"ABCD::"', '"http://a"', '"http:"', '":a"', '"a:"', '"/a"', '"?a:b"', '"#a"', '"a/b:c"', '1', 'null'))),
    s({ f0: { type: ['string', 'integer'] } }, d('"a"', '1', '1.5', '1e5', '1.0', 'null')),
    s({ f0: { type: 'integer' } }, d('1', '1.0', '1e2', '1E2', '-1', '1.5e3', 'true')),
    s({ f0: { type: 5 } }, d('1')), s({ f0: { type: true } }, d('1')), s({ f0: { type: {} } }, d('1')), s({ f0: { type: { 0: 'x' } } }, d('1')), s({ f0: { type: { a: 'x' } } }, d('1')), s({ f0: { type: [] } }, d('1')), s({ f0: { type: [null, 1, ['a', ['b']], {}, true] } }, d('1')), s({ f0: { type: '' } }, d('1')), s({ f0: { type: null } }, d('1')),
    s({ f0: { required: [1, true, 'a'] } }, d({}, { 1: 1 }, { true: 1 })), s({ f0: { required: [null] } }, d({}, { null: 1 })), s({ f0: { required: [{ 0: 'x' }] } }, d({})), s({ f0: { required: [[]] } }, d({})), s({ f0: { required: 'a' } }, d({})),
    s({ f0: { properties: 'ab' } }, d({ 0: 1, a: 1 })), s({ f0: { properties: ['x', { type: 'string' }, false] } }, d({ 0: 1, 1: 1, 2: 1 })), s({ f0: { properties: 5 } }, d({ a: 1 })),
    s({ f0: { patternProperties: 'ab' } }, d({ 0: 1 })), s({ f0: { patternProperties: { '(': false, '^a': true, '(?i)^B': false, '[': { type: 'string' } } } }, d({ a: 1, b: 1, B: 1, c: 1 })),
    s({ f0: { additionalProperties: null } }, d({ a: 1 }, {}, '{"a"}')), s({ f0: { unevaluatedProperties: null } }, d({ a: 1 })), s({ f0: { properties: { a: null } } }, d({ a: 1 }, {})), s({ f0: { properties: { a: 5, b: 'x', c: [] } } }, d({ a: 1, b: 1, c: 1 })),
    s({ f0: { allOf: 5 } }, d('1')), s({ f0: { allOf: [null] } }, d('1')), s({ f0: { allOf: [1, 'a', [], true, false] } }, d('1')), s({ f0: { allOf: 'ab' } }, d('1')), s({ f0: { allOf: {} } }, d('1')), s({ f0: { allowComments: true, allOf: 5 } }, d('1')), s({ f0: { allowComments: 'x', allOf: 0 } }, d('1')), s({ f0: { allowTrailingCommas: false, allOf: 5 } }, d('1')),
    s({ f0: { not: null } }, d('1')), s({ f0: { not: 0 } }, d('1')), s({ f0: { not: '' } }, d('1')), s({ f0: { not: 'x' } }, d('1')), s({ f0: { not: true } }, d('1')), s({ f0: { not: false } }, d('1')), s({ f0: { not: [] } }, d('1')),
    s({ f0: { items: null } }, d([1], [])), s({ f0: { prefixItems: null } }, d([1], [])), s({ f0: { prefixItems: 'ab' } }, d([1, 2, 3])), s({ f0: { prefixItems: 5, items: false } }, d([1, 2])), s({ f0: { prefixItems: {} } }, d([1])), s({ f0: { items: 5 } }, d([1])), s({ f0: { items: 'x' } }, d([1])),
    s({ f0: { contains: false } }, d([1], [])), s({ f0: { contains: true, minContains: 0 } }, d([])), s({ f0: { contains: true, minContains: 2, maxContains: 1, errorMessage: 'E' } }, d([1], [1, 2])), s({ f0: { minContains: 1 } }, d([])), s({ f0: { unevaluatedItems: true } }, d([1])), s({ f0: { unevaluatedItems: null } }, d([1])),
    // Whole-schema oddities.
    ...['5', '0', 'null', 'true', 'false', '""', '"str"', '[]', '[1]', '{"$schema":5}', '{"$schema":""}', '{"$schema":0}', '{"$schema":null}', '{"$schema":"http://json-schema.org/draft-03/schema#"}', '{"$schema":"https://json-schema.org/draft-03/schema"}', '\ufeff{}', '\ufeff{"type":"string"}', '\ufeff \n{"type":"string"', '{"type":"string"', '{"type":"string",}', '{"type":"string"} x', '// c\n{"type":"string"}', 'nonsense', '   ', '\n', '😀', '{"😀":"', '{"allOf":[{"$recursiveRef":"#"},{"$dynamicRef":"#"},{"$recursiveAnchor":true},{"$dynamicAnchor":"x"}]}', '{"$dynamicRef":"#x"}', '{"$recursiveRef":"#","$dynamicRef":"#a","type":"string"}', '{"$recursiveAnchor":true,"definitions":{"a":{"$anchor":"a"}},"$ref":"#a"}', '{"$dynamicAnchor":"x","$ref":"#/missing","type":"string"}'].map((f0) => s({ f0 }, d('1', '"a"', '{}', '{"$schema":"x:y"}', '', '{'))),
    // $ref: pointers, escapes, anchors, chains, cycles.
    s({ f0: { definitions: { 'a/b': { type: 'string' }, 'c~d': { type: 'number' }, 'e f': { type: 'boolean' }, é: { type: 'null' } }, properties: { a: { $ref: '#/definitions/a~1b' }, b: { $ref: '#/definitions/c~0d' }, c: { $ref: '#/definitions/e%20f' }, d: { $ref: '#/definitions/%C3%A9' }, e: { $ref: '#/definitions/é' } } } }, d({ a: 1, b: 'x', c: 1, d: 1, e: 1 })),
    s({ f0: { $ref: '#/%zz' } }, d('1')), s({ f0: { $ref: '#%zz' } }, d('1')), s({ f0: { $ref: '#' } }, d('1')), s({ f0: { $ref: '#', type: 'string' } }, d('1')), s({ f0: { definitions: { a: { $ref: '#/definitions/b' }, b: { $ref: '#/definitions/a' } }, $ref: '#/definitions/a' } }, d('1')),
    s({ f0: { definitions: { a: { $ref: '#' } }, allOf: [{ $ref: '#/definitions/a' }] } }, d('1')), s({ f0: { allOf: [{ $ref: '#' }] } }, d('1')), s({ f0: { anyOf: [{ $ref: '#' }, { type: 'string' }] } }, d('1')), s({ f0: { not: { $ref: '#' } } }, d('1')), s({ f0: { if: { $ref: '#' } } }, d('1')), s({ f0: { items: { $ref: '#' }, type: 'array' } }, d('[[[]]]', '[[[1]]]', '1')), s({ f0: { properties: { a: { $ref: '#' } }, additionalProperties: false } }, d({ a: { a: { b: 1 } } })),
    s({ f0: { description: 'x', definitions: { d: { title: 't', type: 'string' } }, properties: { a: { $ref: '#/definitions/d', title: 'mine', type: 'number', id: 'keep', $id: 'keep' } } } }, d({ a: 1 }, { a: 'x' })),
    s({ f0: { definitions: { d: { id: 'theirs', $id: 'theirs', type: 'string' } }, properties: { a: { $ref: '#/definitions/d' } } } }, d({ a: 1 })),
    s({ f0: { properties: { a: { $ref: '#/description' }, b: { $ref: '#/definitions' }, c: { $ref: '#/n' }, d: { $ref: '#/z' }, e: { $ref: '#/t' }, f: { $ref: '#/f' }, g: { $ref: '#/nul' }, h: { $ref: '#/arr' }, i: { $ref: '#/arr/length' }, j: { $ref: '#/description/length' }, k: { $ref: '#/description/0' }, l: { $ref: '#/toString' }, m: { $ref: '#/constructor/name' }, n: { $ref: '#/n/x' }, o: { $ref: '#/description/x' }, p: { $ref: '#/nul/x' }, q: { $ref: '#/' } }, description: 'de😀', definitions: { type: 'string', x: {} }, n: 5, z: 0, t: true, f: false, nul: null, arr: ['a', { type: 'string' }], '': { type: 'number' } } }, d({ a: 1 }, { b: 1 })),
    s({ f0: { hasOwnProperty: 1, type: 'string', properties: { a: { $ref: '#' } } } }, d({ a: 1 })),
    s({ f0: { definitions: { a: { $id: '#x', type: 'string' }, b: { id: '#y', type: 'number' }, c: { $anchor: 'z', type: 'boolean' }, d: { id: 'notanchor', $anchor: 'w', type: 'null' }, e: { $id: 5, $anchor: 'v', type: 'array' } }, properties: { p: { $ref: '#x' }, q: { $ref: '#y' }, r: { $ref: '#z' }, s: { $ref: '#w' }, t: { $ref: '#v' }, u: { $ref: '#nope' } } } }, d({ p: 1 })),
    s({ f0: { definitions: { a: { $anchor: 'x' }, b: { $anchor: 'x' }, c: { id: '#x' } }, $ref: '#x' } }, d('1')),
    // External references: the merge sees whatever the traversal left.
    s({ f0: { definitions: { late: { $ref: './f1.json' } }, additionalProperties: { $ref: '#/definitions/late' }, properties: { a: { $ref: '#/definitions/late' } } }, f1: { type: 'string' } }, d({ a: 1, b: 1 }, { b: 'x' })),
    s({ f0: { definitions: { late: { $ref: './f1.json', title: 'x' } }, not: { $ref: '#/definitions/late' } }, f1: { type: 'string' } }, d('1', '"a"')),
    s({ f0: { allOf: [{ $ref: './f1.json#/definitions/a' }, { $ref: './f1.json#/definitions/b' }] }, f1: { definitions: { a: { $ref: '#/definitions/b', title: 'a' }, b: { $ref: './f2.json#/definitions/c', required: ['r'] } } }, f2: { definitions: { c: { required: ['s'], properties: { s: { $ref: '#/definitions/d' } } }, d: { type: 'string' } } } }, d({ s: 1 }, {}, { r: 1, s: 'x' })),
    s({ f0: { properties: { a: { $ref: './f1.json#x' } } }, f1: { definitions: { y: { $id: '#x', type: 'string' } } } }, d({ a: 1 })),
    s({ f0: { properties: { a: { $ref: './f1.json#/definitions/none' }, b: { $ref: './f1.json#none' }, c: { $ref: './none.json' }, d: { $ref: './none.json#/x' }, e: { $ref: './none.json#x' } } }, f1: {} }, d({ a: 1 })),
    s({ f0: { properties: { c: { $ref: './none.json' } } } }, d({ c: 1 })), s({ f0: { properties: { d: { $ref: './none.json#x' } } } }, d({ d: 1 })), s({ f0: { properties: { d: { $ref: './none.json#/x' } } } }, d({ d: 1 })),
    s({ f0: { $ref: './f1.json' }, f1: '{"type":"string"' }, d('1')), s({ f0: { $ref: './f1.json#/type' }, f1: '\ufeff{"type":"string"}' }, d('1')), s({ f0: { $ref: './f1.json' }, f1: 'null' }, d('1')), s({ f0: { $ref: './f1.json' }, f1: '5' }, d('1')), s({ f0: { $ref: './f1.json#/a' }, f1: 'null' }, d('1')), s({ f0: { $ref: './f1.json' }, f1: '"ab"' }, d('1')), s({ f0: { $ref: './f1.json' }, f1: '[{"type":"string"}]' }, d('1')), s({ f0: { $ref: './f1.json' }, f1: '{"$ref":"#/definitions/a","definitions":{"a":{"type":"string"}}}' }, d('1')),
    s({ f0: { $ref: '../theme/f1.json#/definitions/a' }, f1: { definitions: { a: { type: 'string' } } } }, d('1')), s({ f0: { $ref: 'f1.json' }, f1: { type: 'string' } }, d('1')), s({ f0: { $ref: '/nowhere/f1.json' }, f1: {} }, d('1')), s({ f0: { $ref: `${SCHEMA_ROOT}/x/f1.json#/type` }, f1: { type: 'string' } }, d('1')), s({ f0: { $ref: 'HTTPS://RAW.githubusercontent.com/Shopify/theme-liquid-docs/main/schemas/x/f1.json' }, f1: { type: 'string' } }, d('1')), s({ f0: { $ref: 'https://json-schema.org/draft-07/schema' } }, d('1', '{"type":5}', '{"type":"string"}')), s({ f0: { $ref: 'http://json-schema.org/draft-04/schema#/definitions/positiveInteger' } }, d('-1', '1')), s({ f0: { $ref: 'https://json-schema.org/draft/2020-12/schema' } }, d('1', '{"type":5}')), s({ f0: { $ref: 'a#b#c' } }, d('1')), s({ f0: { $ref: 'http://[bad/#x' } }, d('1')), s({ f0: { $ref: '//[bad' } }, d('1')),
    s({ f0: { $ref: 5 } }, d('1')), s({ f0: { $ref: true } }, d('1')), s({ f0: { $ref: [] } }, d('1')), s({ f0: { $ref: {} } }, d('1')), s({ f0: { $ref: null } }, d('1')), s({ f0: { $ref: 0 } }, d('1')), s({ f0: { $ref: '' } }, d('1')),
    // Associations.
    s({ f0: { required: ['a'] }, f1: { required: ['b'] } }, [...d({}), ...d({})], { f0: ['t/*.json'], f1: ['t/*.json', '!x'] }),
    s({ f0: { required: ['a'] }, f1: { required: ['b'], $ref: './f0.json' } }, d({ a: 1 }), { f0: ['t/*.json'], f1: ['t/*.json'] }),
    s({ f0: { required: ['a'] } }, d({}), { f0: ['!t/*.json'] }), s({ f0: { required: ['a'] } }, d({}), { f0: ['*.json', '!t/doc.json'] }), s({ f0: { required: ['a'] } }, d({}), { f0: ['/t/*.json'] }), s({ f0: { required: ['a'] } }, d({}), { f0: ['**/doc.json'] }), s({ f0: { required: ['a'] } }, d({}), { f0: ['t/**'] }), s({ f0: { required: ['a'] } }, d({}), { f0: ['{t,u}/d?c.json'] }), s({ f0: { required: ['a'] } }, d({}), { f0: ['t/[a-e]oc.json'] }), s({ f0: { required: ['a'] } }, d({}), { f0: ['[bad'] }), s({ f0: { required: ['a'] } }, d({}), { f0: [''] }), s({ f0: { required: ['a'] } }, d({}), { f0: ['!'] }), s({ f0: { required: ['a'] } }, d({}), { f0: [] }), s({ f0: { required: ['a'] } }, d({}), { f0: ['t/a,b.json', 'w/t/doc.json'] }), s({ f0: { required: ['a'] } }, d({}), { f0: ['file:///w/t/doc.json'] }), s({ f0: { required: ['a'] } }, d({}), { f0: ['t/doc*'] }), s({ f0: { required: ['a'] } }, d({}), { f0: ['t/*.*'] }),
  ].map((x) => ({ ...x, uri }));
}

async function runHandwritten() {
  const dir = path.join(OUT, 'handwritten');
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  for (const scenario of handwrittenScenarios()) {
    const names = Object.keys(scenario.files);
    for (const name of names) {
      const v = scenario.files[name];
      const text = typeof v === 'string' ? JSON.parse(`"${v.replace(/\n/g, '\\n').replace(/"/g, '\\"').replace(/\\\\ufeff/g, '\\ufeff')}"`) : JSON.stringify(v);
      fs.writeFileSync(path.join(dir, `${name}.json`), text);
    }
    const manifest = path.join(dir, 'manifest.json');
    fs.writeFileSync(manifest, JSON.stringify({ schemas: names.map((n) => ({ uri: `x/${n}.json`, ...(scenario.match[n] ? { fileMatch: scenario.match[n] } : {}) })) }));
    // Fresh validators per scenario; each runs its documents twice, to see cached state.
    const cases = [...scenario.docs, ...scenario.docs].map((json) => ({ uri: scenario.uri, json }));
    await runBatch('handwritten', 'theme', dir, manifest, cases);
  }
}

// ---- URL ----

function urlCases() {
  const r = rng(SEED + 99);
  const inputs = ['', ' ', 'a', 'a b', './a', '../a', '../../../a', '/a', '//a', '///a', '////a', '?q', '#f', 'a?b#c', 'a#b?c', '.', '..', '%2e', '%2E%2e/x', '.%2e/', 'a/./b', 'a/../b', 'a\\b', '\\a', '\\\\a', 'é', '日本', '😀', 'a b?c d#e f', 'a"b<c>d`e{f}g^h|i', '?a"b<c>d\'e', '#a"b<c>d`e', 'http://x', 'http:', 'http:/x', 'http:x', 'file:x', 'file:/x', 'file://x/y', 'file:///x', 'file:', 'c:', 'C:/x', 'C|/x', '/C:/x', 'c:\\x', '//[::1]/x', '//[bad', '//[::1', '//a b/', '//a%20b/', '//a:80/', '//a:x/', '//a:99999/', '//u:p@h/', '//@h/', '//u@/', '//localhost/x', '//127.1/x', '//1.2.3.4.5/x', '//0x7f.1/', '//256.1.1.1/', '//EXAMPLE.com/x', '//é.com/', '//a.b./', 'x:y', 'x:/y', 'x://y', 'x://y z/', 'x:///y', 'x://[::1]/', 'a\tb', 'a\nb', ' a ', '\u0001a\u0001', 'a/%zz', 'a/%41', 'foo:bar?x#y', 'mailto:a@b', 'data:text/plain,x'];
  const bases = ['file:///a/b/c.json', 'file:///c:/a/b.json', 'file://host/a/b', 'file:///', 'file:', 'https://raw.githubusercontent.com/Shopify/theme-liquid-docs/main/schemas/theme/section.json', 'http://a:8080/b/c?d#e', 'https://u:p@h/x/', 'x:y', 'x:/y/z', 'x://h/y/z', 'mailto:a@b', 'not a uri', '', '/abs', 'schemaservice://combinedSchema/x'];
  const cases = [];
  for (const b of bases) for (const i of inputs) cases.push([i, b]);
  for (let k = 0; k < 4000; k++) cases.push([pick(r, inputs) + pick(r, inputs), pick(r, bases)]);
  return cases;
}

function runUrlGroup() {
  const cases = urlCases();
  const file = path.join(OUT, 'url-cases.ndjson');
  fs.writeFileSync(file, cases.map((c) => JSON.stringify(c)).join('\n'));
  const lines = execFileSync(BIN, ['url', '--cases', file], { maxBuffer: 1 << 30, encoding: 'utf8' }).split('\n');
  const g = group('url');
  cases.forEach(([input, base], i) => {
    let expected = null;
    try {
      expected = new URL(input, base).toString();
    } catch {
      expected = null;
    }
    const got = JSON.parse(lines[i]);
    g.documents++;
    const nonAsciiHost = /\/\/[^/?#]*[^\x00-\x7f]/.test(input) || /^[a-z]+:\/*[^/?#]*[^\x00-\x7f]/i.test(input);
    if (expected !== got) {
      if (nonAsciiHost) {
        category('url-idna', 'a non-ASCII host needs IDNA; the port keeps it lowercased').mismatched++;
        return;
      }
      g.diffs++;
      diffs++;
      const text = `[url] new URL(${JSON.stringify(input)}, ${JSON.stringify(base)}): oracle ${JSON.stringify(expected)} lq ${JSON.stringify(got)}\n`;
      fs.appendFileSync(DIFF_FILE, text);
      if (shown.length < SHOW) shown.push(text);
    }
  });
}

// ---- Main ----

const THEME_MANIFEST = path.join(DOCS, 'manifest_theme.json');
const APP_MANIFEST = path.join(DOCS, 'manifest_theme_app_extension.json');

async function main() {
  const real = realDocuments();
  console.log(`[jsonschema] base documents: ${real.length}; setting types: ${settingTypes.length}`);
  const modes = [['theme', THEME_MANIFEST], ['app', APP_MANIFEST]];

  if (enabled('real')) {
    for (const [mode, manifest] of modes) await runBatch(`real:${mode}`, mode, DOCS, manifest, real);
  }

  if (enabled('mutated')) {
    for (const [mode, manifest] of modes) {
      let cases = [];
      for (const doc of real) {
        const size = doc.json.length;
        const count = size > 60000 ? Math.ceil(MUTATIONS / 12) : size > 15000 ? Math.ceil(MUTATIONS / 3) : MUTATIONS;
        cases.push(...mutations(doc, count, SEED + (mode === 'app' ? 1000 : 0)));
        if (cases.length >= 4000) {
          await runBatch(`mutated:${mode}`, mode, DOCS, manifest, cases);
          cases = [];
        }
      }
      await runBatch(`mutated:${mode}`, mode, DOCS, manifest, cases);
    }
  }

  if (enabled('settings')) {
    for (const [mode, manifest] of modes) await runBatch(`settings:${mode}`, mode, DOCS, manifest, settingDocuments(SEED + 7));
  }

  if (enabled('order')) {
    const sample = [...real.filter((d) => d.json.length < 20000).filter((_, i) => i % 7 === 0).slice(0, 150), ...settingDocuments(SEED + 8).filter((_, i) => i % 40 === 0)];
    const firsts = KNOWN_URIS.map((u) => ({ uri: 'file:///first/x.json', json: JSON.stringify({ $schema: u }) }));
    let baseline = null;
    let orderSensitive = 0;
    for (const first of firsts) {
      const results = await runBatch('order', 'theme', DOCS, THEME_MANIFEST, [first, ...sample]);
      const key = results.slice(1).map((x) => JSON.stringify(x));
      if (baseline) orderSensitive += key.filter((k, i) => k !== baseline[i]).length;
      else baseline = key;
    }
    console.log(`[jsonschema] upstream results that depend on which schema resolved first: ${orderSensitive}`);
  }

  if (enabled('traps')) {
    for (const [mode, manifest] of modes) await runBatch(`traps:${mode}`, mode, DOCS, manifest, trapDocuments());
  }

  if (enabled('handwritten')) await runHandwritten();

  if (enabled('synth')) {
    const dir = path.join(OUT, 'synth');
    fs.rmSync(dir, { recursive: true, force: true });
    fs.mkdirSync(dir, { recursive: true });
    for (let b = 0; b < SYNTH_BATCHES; b++) {
      const batch = synthBatch(SEED * 7919 + b);
      batch.files.forEach((text, i) => fs.writeFileSync(path.join(dir, `f${i}.json`), text));
      const manifest = path.join(dir, 'manifest.json');
      fs.writeFileSync(manifest, JSON.stringify(batch.manifest));
      await runBatch('synth', 'theme', dir, manifest, batch.cases);
    }
  }

  if (enabled('url')) runUrlGroup();

  for (const [name, g] of Object.entries(groups)) {
    console.log(`[jsonschema]   ${name}: documents=${g.documents} diffs=${g.diffs}`);
  }
  for (const [name, c] of Object.entries(categories)) {
    console.log(`[jsonschema]   category ${name}: count=${c.count} mismatched=${c.mismatched} (${c.reason})`);
  }
  const missed = TEMPLATES.filter((t) => templateHits.get(t) === 0);
  console.log(`[jsonschema] coverage: ${TEMPLATES.length - missed.length}/${TEMPLATES.length} upstream message templates hit, ${customMessages} schema-authored messages, ${rejections.size} distinct rejections`);
  for (const t of missed) console.log(`[jsonschema]   never hit: ${t.source}`);
  fs.writeFileSync(path.join(OUT, 'coverage.txt'), [...[...templateHits].map(([t, n]) => `${n}\t${t.source}`), ...[...rejections].map(([m, n]) => `${n}\t${m}`)].join('\n'));
  for (const text of shown) console.log(text);
  if (diffs > 0) console.log(`[jsonschema] all diffs: ${DIFF_FILE}`);
  console.log(`[jsonschema] documents=${documents} diffs=${diffs}`);
  process.exit(diffs > 0 ? 1 : 0);
}

await main();
