#!/usr/bin/env node
// Compares engine dependencies that have no check of their own to prove them yet:
// the theme graph, doc definitions, block parameters and CSS classes, on the themes.
import fs from 'node:fs';
import path from 'node:path';
import { THEMES, oracleRequire, runLq, snippet, firstDiff } from './lib.mjs';

const tc = oracleRequire('@shopify/theme-check-node');
const { buildThemeGraph } = oracleRequire('@shopify/theme-graph');
const { makeGetBlockParameters } = oracleRequire('@shopify/theme-check-common');
const { URI } = oracleRequire('vscode-uri');

let total = 0;
for (const [name, root] of Object.entries(THEMES)) {
  const { theme, config } = await tc.getThemeAndConfig(root);
  const validator = await tc.JSONValidator.create(
    { schemas: async (mode) => docsSchemas(mode) },
    config,
  );
  const isValid = validator.isValid;
  const memo = new Map();
  const schemaOf = (kind) => async (n) => {
    const source = theme.find((s) => (kind === 'block' ? tc.isBlock(s.uri) : tc.isSection(s.uri)) && tc.path.basename(s.uri, '.liquid') === n);
    if (!source) return undefined;
    const key = source.uri;
    if (!memo.has(key)) memo.set(key, tc.toSchema(config.context, source.uri, source, isValid));
    return memo.get(key);
  };
  const getBlockSchema = schemaOf('block');
  const getSectionSchema = schemaOf('section');
  const refs = (list) =>
    list
      .map((r) => `${{ direct: 'Direct', indirect: 'Indirect', preset: 'Preset' }[r.type]} ${r.source.uri} -> ${r.target.uri}`)
      .sort();
  const want = {};
  try {
    const g = await buildThemeGraph(config.rootUri, {
      fs: tc.NodeFileSystem,
      getSectionSchema,
      getBlockSchema,
      getWebComponentDefinitionReference: () => undefined,
      mode: config.context,
    });
    want.modules = {};
    for (const [u, m] of Object.entries(g.modules)) {
      want.modules[u] = { exists: !!m.exists, dependencies: refs(m.dependencies), references: refs(m.references) };
    }
  } catch (e) {
    want.graphError = String(e).replace(/^\w*Error: /, '');
  }

  want.css = {};
  for (const s of theme) {
    if (s.uri.endsWith('.liquid')) want.css[s.uri] = [...(await tc.extractCSSClassesFromLiquidUri(s.uri, tc.NodeFileSystem))].sort();
  }
  try {
    for (const [u] of await tc.NodeFileSystem.readDirectory(tc.path.join(config.rootUri, 'assets'))) {
      if (u.endsWith('.css')) want.css[u] = [...(await tc.extractCSSClassesFromAssetUri(u, tc.NodeFileSystem))].sort();
    }
  } catch {}

  want.docs = {};
  const docOf = new Map();
  const rootUri = URI.file(root).toString();
  for (const s of theme) {
    if (!tc.filePathSupportsLiquidDoc(s.uri) || s.ast instanceof Error) continue;
    const rel = tc.path.relative(rootUri, s.uri) && tc.path.relative(s.uri, rootUri);
    const d = tc.extractDocDefinition(s.uri, s.ast);
    docOf.set(rel, d);
    want.docs[rel] = { hasDoc: !!d.liquidDoc };
    if (d.liquidDoc) {
      want.docs[rel].parameters = (d.liquidDoc.parameters ?? []).map((p) => [p.name, p.type, p.description, p.required]);
      want.docs[rel].examples = (d.liquidDoc.examples ?? []).map((e) => e.content);
      want.docs[rel].description = d.liquidDoc.description?.content ?? null;
    }
  }

  want.blockParameters = {};
  const getParams = makeGetBlockParameters({ getBlockSchema, getDocDefinition: async (r) => docOf.get(r) });
  for (const s of theme) {
    if (!tc.isBlock(s.uri) || !s.uri.includes('/blocks/')) continue;
    const n = tc.path.basename(s.uri, '.liquid');
    const p = await getParams(n);
    want.blockParameters[n] = p ? [...p.values()].map((x) => [x.name, x.type ?? null, x.required]) : null;
  }

  const got = JSON.parse(runLq(['debug-engine', '--path', root, '--upstream-bugs']));
  for (const section of ['graphError', 'modules', 'css', 'docs', 'blockParameters']) {
    const d = firstDiff(want[section], got[section], section);
    const size = want[section] && typeof want[section] === 'object' ? Object.keys(want[section]).length : 0;
    console.log(`[engine/${name}] ${section}: entries=${size} ${d ? `DIFF at ${d.path}: oracle=${snippet(d.oracle)} lq=${snippet(d.lq)}` : 'same'}`);
    if (d) total++;
  }
}
console.log(`[engine] diffs=${total}`);
process.exit(total ? 1 : 0);

function docsSchemas(mode) {
  const dir = path.resolve(path.dirname(new URL(import.meta.url).pathname), '../../data/docs');
  const manifest = JSON.parse(fs.readFileSync(path.join(dir, mode === 'app' ? 'manifest_theme_app_extension.json' : 'manifest_theme.json'), 'utf8'));
  return manifest.schemas.map((s) => ({
    uri: `https://raw.githubusercontent.com/Shopify/theme-liquid-docs/main/schemas/${s.uri}`,
    fileMatch: s.fileMatch,
    schema: fs.readFileSync(path.join(dir, path.basename(s.uri)), 'utf8'),
  }));
}
