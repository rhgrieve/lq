// Differential test of crates/lq-theme's config loading and YAML port against
// the oracle's theme-check-node and yaml packages.
import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { ROOT, THEMES, GEN, oracleRequire, rng } from './lib.mjs';

process.removeAllListeners('warning');
process.on('warning', () => {});

const PROFILE = process.env.LQ_PROFILE ?? 'fast';
const BIN = path.join(ROOT, 'target', PROFILE, 'lq-theme-debug');
const WORK = path.join(GEN, 'config-cases');
const DIFFS = path.join(GEN, 'config-diffs.txt');
const REGISTRY = path.join(GEN, 'registry.json');

const common = oracleRequire('@shopify/theme-check-common');
const node = oracleRequire('@shopify/theme-check-node');
const YAML = oracleRequire('yaml');

function dumpProp(p) {
  const o = p.options;
  const out = { type: o.type };
  if (o.defaultValue !== undefined) out.default = o.defaultValue;
  if (o.optional) out.optional = true;
  if (o.properties) out.properties = dumpSchema(o.properties);
  if (o.itemType) out.itemType = dumpProp(o.itemType);
  return out;
}
function dumpSchema(s) {
  return Object.fromEntries(Object.entries(s ?? {}).map(([k, p]) => [k, dumpProp(p)]));
}
export function writeRegistry() {
  fs.mkdirSync(GEN, { recursive: true });
  const reg = common.allChecks.map((c) => ({
    code: c.meta.code,
    aliases: c.meta.aliases ?? [],
    schema: dumpSchema(c.meta.schema),
  }));
  fs.writeFileSync(REGISTRY, JSON.stringify(reg));
  return reg;
}

async function oracleConfig(configPath, root) {
  const lines = [];
  const orig = console.error;
  console.error = (...a) => lines.push(a.join(' '));
  try {
    const c = await node.loadConfig(configPath, root);
    return {
      out: JSON.stringify({
        context: c.context,
        settings: c.settings,
        checks: c.checks.map((k) => k.meta.code),
        rootUri: c.rootUri,
        ignore: c.ignore,
      }),
      stderr: lines,
    };
  } catch (e) {
    return { out: JSON.stringify({ error: { name: e.name, message: e.message } }), stderr: lines };
  } finally {
    console.error = orig;
  }
}

function lqConfig(configPath, root) {
  const args = ['config', '--root', root, '--registry', REGISTRY];
  if (configPath !== undefined) args.push('--config', configPath);
  const r = spawnSync(BIN, args, { encoding: 'utf8', maxBuffer: 1 << 28 });
  return { out: r.stdout.trim(), stderr: r.stderr.split('\n').filter(Boolean) };
}

const stderrFilter = (lines) =>
  lines.filter((l) => !l.startsWith('[lq] note') && !l.startsWith('Error loading')).join('\n');

function writeTree(dir, files) {
  for (const [rel, content] of Object.entries(files)) {
    const p = path.join(dir, rel);
    fs.mkdirSync(path.dirname(p), { recursive: true });
    if (typeof content === 'object' && content.symlink) fs.symlinkSync(content.symlink, p);
    else fs.writeFileSync(p, content);
  }
}

const CFG = '.theme-check.yml';

function handCases(reg) {
  const c = (name, files, extra = {}) => ({ name, files, ...extra });
  const y = (name, text, extra = {}) => c(name, { [CFG]: text, ...(extra.files ?? {}) }, extra);
  const out = [
    c('no-config', { 'a.liquid': '' }),
    c('app-toml', { 'shopify.app.toml': '' }),
    c('extension-toml', { 'x.extension.toml': '' }),
    c('other-toml', { 'x.toml': '' }),
    c('config-is-dir', { [`${CFG}/x`]: '' }),
    c('missing-root', {}, { rootRel: 'nope' }),
    c('missing-config', {}, { config: 'nope.yml' }),
    c('root-is-file', { f: '' }, { rootRel: 'f' }),
    y('empty', ''),
    y('whitespace', '  \n\t\n'),
    y('comment-only', '# nothing\n'),
    y('scalar', 'hello\n'),
    y('number', '12\n'),
    y('array', '- a\n- b\n'),
    y('null-doc', '~\n'),
    y('bom', '\ufeffignore:\n  - a\n'),
    y('crlf', 'ignore:\r\n  - a\r\nAssetPreload:\r\n  enabled: false\r\n'),
    y('unquoted-star', 'ignore:\n  - *.foo\n'),
    y('unquoted-star-inline', 'ignore: *.foo\n'),
    y('unquoted-star-setting', 'AssetPreload:\n  ignore:\n    - *.liquid\n'),
    y('quoted-star', "ignore:\n  - '*.foo'\n"),
    y('anchors', 'AssetPreload: &a\n  enabled: false\nAssetSizeCSS: *a\n'),
    y('anchors-ignore', 'ignore: &i\n  - a\nAssetPreload:\n  ignore: *i\n'),
    y('merge-key', 'AssetPreload: &a\n  enabled: false\nAssetSizeCSS:\n  <<: *a\n'),
    y('tab-indent', 'AssetPreload:\n\tenabled: false\n'),
    y('dup-key', 'ignore: []\nignore: []\n'),
    y('dup-nested', 'AssetPreload:\n  enabled: true\n  enabled: false\n'),
    y('unterminated', "ignore: 'abc\n"),
    y('unterminated-dq', 'ignore: "abc\n'),
    y('bad-flow', 'ignore: [a, b\n'),
    y('bad-flow-map', 'AssetPreload: {enabled: true\n'),
    y('bad-compact', 'a: b: c\n'),
    y('bad-seq', '- a\nb: 1\n'),
    y('bad-indent', 'a:\n    b: 1\n  c: 2\n'),
    y('multi-doc', 'ignore: []\n---\nignore: []\n'),
    y('doc-markers', '---\nignore: []\n...\n'),
    y('directive', '%YAML 1.2\n---\nignore: []\n'),
    y('bad-escape', 'ignore: "\\q"\n'),
    y('long-line-error', `ignore: "${'x'.repeat(120)}\\q"\n`),
    y('unicode-error', 'ignore: "é\\q"\n'),
    y('at-start', 'ignore: @foo\n'),
    y('backtick-start', 'ignore: `foo\n'),
    y('percent-start', 'ignore: %foo\n'),
    y('pipe-block', 'ignore:\n  - |\n    a\n'),
    y('fold-block', 'ignore:\n  - >-\n    a\n    b\n'),
    y('include-categories', 'include_categories: [liquid]\nexclude_categories: [json]\n'),
    y('include-categories-null', 'include_categories: ~\n'),
    y('exclude-categories-empty', "exclude_categories: ''\n"),
    y('extends-null', 'extends: ~\n'),
    y('extends-empty-string', "extends: ''\n"),
    y('extends-empty-list', 'extends: []\n'),
    y('ignore-null', 'ignore: ~\n'),
    y('root-empty', "root: ''\n"),
    y('root-null', 'root: ~\n'),
    y('context-null', 'context: ~\n'),
    y('require-null', 'require: ~\n'),
    y('non-object-check-num', 'AssetPreload: 1\n'),
    y('non-object-check-null', 'AssetPreload: ~\n'),
    y('non-object-check-str', 'AssetPreload: yes\n'),
    y('non-object-check-list', 'AssetPreload: []\n'),
    y('unknown-check', 'NotACheck:\n  enabled: true\n'),
    y('unknown-check-disabled', 'NotACheck:\n  enabled: false\n'),
    y('numeric-check-keys', 'z:\n  enabled: true\n2:\n  enabled: true\n1:\n  enabled: true\n'),
    y('null-key', '~:\n  enabled: true\n'),
    y('bool-key', 'true:\n  enabled: true\n'),
    y('float-key', '1.5:\n  enabled: true\n'),
    y('collection-key', '[a, b]:\n  enabled: true\n'),
    y('enabled-zero', 'AssetPreload:\n  enabled: 0\n'),
    y('enabled-str', 'AssetPreload:\n  enabled: "yes"\n'),
    y('enabled-empty-str', "AssetPreload:\n  enabled: ''\n"),
    y('enabled-null', 'AssetPreload:\n  enabled: ~\n'),
    y('enabled-absent', 'AssetPreload:\n  severity: error\n'),
    y('enabled-true', 'AssetPreload:\n  enabled: true\n'),
    y('all-disabled-undefined', 'extends: theme-check:nothing\nAssetPreload:\n  enabled: false\nAssetSizeCSS:\n  enabled: false\n  thresholdInBytes: 3\n'),
    y('alias-sev', 'extends: nothing\nSyntaxError:\n  enabled: true\n  severity: error\n'),
    y('alias-second', 'extends: nothing\nHtmlParsingError:\n  enabled: true\n'),
    y('alias-disabled', 'extends: nothing\nSyntaxError:\n  enabled: false\n'),
    y('alias-and-code', 'extends: nothing\nValidJSON:\n  enabled: true\nValidJson:\n  severity: info\n'),
    y('alias-and-code-off', 'extends: nothing\nValidJSON:\n  enabled: false\nValidJson:\n  enabled: true\n'),
    y('alias-order', 'extends: nothing\nValidJson:\n  severity: info\nValidJSON:\n  enabled: true\n'),
    y('alias-multi', 'extends: all\nDeprecatedFilters:\n  severity: info\nDeprecatedTags:\n  enabled: false\nAssetUrlFilters:\n  ignore: [a]\n'),
    y('sev-names', ['suggestion', 'style', 'error', 'warning', 'info'].map((s, i) => `Check${i}:\n  severity: ${s}\n`).join('')),
    y('sev-nums', 'AssetPreload:\n  severity: 0\nAssetSizeCSS:\n  severity: 1\nValidJSON:\n  severity: 2\n'),
    y('sev-3', 'AssetPreload:\n  severity: 3\n'),
    y('sev-neg', 'AssetPreload:\n  severity: -1\n'),
    y('sev-float', 'AssetPreload:\n  severity: 1.5\n'),
    y('sev-1.0', 'AssetPreload:\n  severity: 1.0\n'),
    y('sev-neg0', 'AssetPreload:\n  severity: -0\n'),
    y('sev-str', 'AssetPreload:\n  severity: bad\n'),
    y('sev-str-num', 'AssetPreload:\n  severity: "1"\n'),
    y('sev-upper', 'AssetPreload:\n  severity: ERROR\n'),
    y('sev-null', 'AssetPreload:\n  severity: ~\n'),
    y('sev-list', 'AssetPreload:\n  severity: [a, ~, 1]\n'),
    y('sev-map', 'AssetPreload:\n  severity: {a: 1}\n'),
    y('sev-bool', 'AssetPreload:\n  severity: true\n'),
    y('sev-empty-string', "AssetPreload:\n  severity: ''\n"),
    y('sev-unknown-check', 'NotACheck:\n  severity: nope\n'),
    y('ignore-str', 'ignore: a/b\n'),
    y('ignore-num', 'ignore: 5\n'),
    y('ignore-bool', 'ignore: true\n'),
    y('ignore-map', 'ignore: {a: 1}\n'),
    y('ignore-mixed', 'ignore: [a, 1, ~, [b]]\n'),
    y('check-ignore-list', 'AssetPreload:\n  ignore: [a, b]\n'),
    y('check-ignore-str', 'AssetPreload:\n  ignore: a\n'),
    y('check-ignore-num', 'AssetPreload:\n  ignore: 1\n'),
    y('check-ignore-null', 'AssetPreload:\n  ignore: ~\n'),
    y('check-ignore-mixed', 'AssetPreload:\n  ignore: [a, 1]\n'),
    y('check-ignore-disabled', 'AssetPreload:\n  enabled: false\n  ignore: 1\n'),
    y('snake-keys', 'AssetSizeCSS:\n  threshold_in_bytes: 5\n  some_other_key: 1\n  _leading: 2\n  a__b: 3\n  a_B: 4\n  trailing_: 5\n  a_1: 6\n'),
    y('snake-both', 'AssetSizeCSS:\n  threshold_in_bytes: 5\n  thresholdInBytes: 6\n'),
    y('camel-both-reverse', 'AssetSizeCSS:\n  thresholdInBytes: 6\n  threshold_in_bytes: 5\n'),
    y('superfluous', 'AssetPreload:\n  foo: 1\n  bar_baz: 2\n  toString: 3\n  constructor: 4\n'),
    y('wrong-number', 'AssetSizeCSS:\n  thresholdInBytes: abc\n'),
    y('wrong-number-null', 'AssetSizeCSS:\n  thresholdInBytes: ~\n'),
    y('wrong-number-list', 'AssetSizeCSS:\n  thresholdInBytes: []\n'),
    y('wrong-disabled', 'AssetSizeCSS:\n  enabled: false\n  thresholdInBytes: abc\n'),
    y('root-rel', 'root: dist\n'),
    y('root-dot', 'root: ./dist\n'),
    y('root-up', 'root: ../other\n'),
    y('root-trailing', 'root: dist/\n'),
    y('root-abs', 'root: /abs\n'),
    y('root-num', 'root: 5\n'),
    y('root-bool', 'root: true\n'),
    y('root-map', 'root: {a: 1}\n'),
    y('root-list', 'root: [dist]\n'),
    y('root-list-num', 'root: [5]\n'),
    y('root-list-multi', 'root: [a, b]\n'),
    y('root-list-empty', 'root: []\n'),
    y('root-spaces', 'root: "my dist"\n'),
    y('root-unicode', 'root: "dïst#x?y%z"\n'),
    y('context-theme', 'context: theme\n'),
    y('context-app', 'context: app\n'),
    y('context-bad', 'context: nope\n'),
    y('context-num', 'context: 1\n'),
    y('context-list', 'context: [theme]\n'),
    y('require-rel', 'require: ./nope.js\n'),
    y('require-abs', 'require: [/nope/x.js]\n'),
    y('require-num', 'require: 5\n'),
    y('require-null-item', 'require: [~]\n'),
    y('require-bool-item', 'require: [true]\n'),
    y('require-map-item', 'require: [{a: 1}]\n'),
    y('require-list-item', 'require: [[a]]\n'),
    y('require-module-missing', 'require: nope-module\n'),
    y('require-module-ok', 'require: reqmod\n', { files: { 'node_modules/reqmod/index.js': 'module.exports={checks:[]}' } }),
    y('extends-num', 'extends: 5\n'),
    y('extends-true', 'extends: true\n'),
    y('extends-null-item', 'extends: [~]\n'),
    y('extends-map-item', 'extends: [{a: 1}]\n'),
    y('extends-list-item', 'extends: [[a]]\n'),
    ...[':default', ':nothing', ':theme_app_extension', ':theme_app_extensions', ':all', ':recommended', ':theme-app-extension', ':theme-app-extensions', ':foo', 'default', 'nothing', 'all', 'recommended', 'theme_app_extension', 'theme-app-extension', 'theme-check:nothing', 'theme-check:all', 'theme-check:recommended', 'theme-check:theme-app-extension', 'theme-check:foo', 'foo-module', './nope.yml', '../nope.yml', '/nope/abs.yml'].map((e) => y(`extends-${e}`, `extends: ${JSON.stringify(e)}\n`)),
    y('extends-list', 'extends: [nothing, all]\nAssetPreload:\n  enabled: false\n'),
    y('extends-rel', 'extends: ./a.yml\n', { files: { 'a.yml': 'ignore: [a]\nAssetPreload:\n  severity: error\n' } }),
    y('extends-rel-subdir', 'extends: ./sub/a.yml\n', { files: { 'sub/a.yml': 'extends: ../b.yml\nignore: [a]\n', 'b.yml': 'ignore: [b]\nroot: rr\n' } }),
    y('extends-dotfile', 'extends: .other.yml\n', { files: { '.other.yml': 'ignore: [dot]\n' } }),
    y('extends-norm', 'extends: ./x/../a.yml\n', { files: { 'a.yml': 'ignore: [a]\n', 'x/keep': '' } }),
    y('extends-dir', 'extends: ./sub\n', { files: { 'sub/a': '' } }),
    y('extends-non-root-no-default', 'extends: ./a.yml\n', { files: { 'a.yml': 'AssetPreload:\n  enabled: true\n' } }),
    y('cycle-self', 'extends: ./.theme-check.yml\n'),
    y('cycle-2', 'extends: ./a.yml\n', { files: { 'a.yml': 'extends: ./.theme-check.yml\n' } }),
    y('cycle-3', 'extends: ./a.yml\n', { files: { 'a.yml': 'extends: ./b.yml\n', 'b.yml': 'extends: ./a.yml\n' } }),
    y('diamond', 'extends: [./a.yml, ./b.yml]\n', { files: { 'a.yml': 'extends: ./c.yml\nignore: [a]\n', 'b.yml': 'extends: ./c.yml\nignore: [b]\n', 'c.yml': 'ignore: [c]\nAssetPreload:\n  severity: info\n' } }),
    y('merge-deep', 'extends: ./a.yml\nX:\n  n:\n    b: 2\n  l: [2]\n', { files: { 'a.yml': 'X:\n  n:\n    a: 1\n  l: [1]\n  o: {x: 1}\nY: {enabled: false}\n' } }),
    y('merge-replace-object', 'extends: ./a.yml\nX:\n  o: 1\n', { files: { 'a.yml': 'X:\n  o: {x: 1}\n' } }),
    y('context-merge', 'extends: ./a.yml\n', { files: { 'a.yml': 'context: app\n' } }),
    y('context-override', 'extends: ./a.yml\ncontext: theme\n', { files: { 'a.yml': 'context: app\n' } }),
    y('root-merge', 'extends: [./a.yml, ./b.yml]\n', { files: { 'a.yml': 'root: a\n', 'b.yml': 'root: b\n' } }),
    y('root-merge-override', 'extends: ./a.yml\nroot: c\n', { files: { 'a.yml': 'root: a\n' } }),
    y('require-merge', 'extends: ./a.yml\nrequire: /n/b.js\n', { files: { 'a.yml': 'require: /n/a.js\n' } }),
    y('node-module-main', 'extends: nm-main\n', { files: { 'node_modules/nm-main/package.json': '{"main":"cfg.yml"}', 'node_modules/nm-main/cfg.yml': 'ignore: [nm]\n' } }),
    y('node-module-sub', 'extends: nm-sub/x.yml\n', { files: { 'node_modules/nm-sub/x.yml': 'ignore: [nmsub]\n' } }),
    y('node-module-scoped', 'extends: "@acme/rec/x.yml"\n', { files: { 'node_modules/@acme/rec/x.yml': 'ignore: [scoped]\n' } }),
    y('node-module-scoped-main', 'extends: "@acme/rec"\n', { files: { 'node_modules/@acme/rec/package.json': '{"main":"./lib/m.yml"}', 'node_modules/@acme/rec/lib/m.yml': 'ignore: [scoped-main]\n' } }),
    y('node-module-exports-str', 'extends: nm-e\n', { files: { 'node_modules/nm-e/package.json': '{"exports":"./e.yml","main":"nope"}', 'node_modules/nm-e/e.yml': 'ignore: [e]\n' } }),
    y('node-module-exports-map', 'extends: nm-e\n', { files: { 'node_modules/nm-e/package.json': '{"exports":{".":{"import":"./i.yml","require":"./r.yml"}}}', 'node_modules/nm-e/r.yml': 'ignore: [r]\n' } }),
    y('node-module-exports-default', 'extends: nm-e\n', { files: { 'node_modules/nm-e/package.json': '{"exports":{".":{"default":"./d.yml"}}}', 'node_modules/nm-e/d.yml': 'ignore: [d]\n' } }),
    y('node-module-exports-sub', 'extends: nm-e/a\n', { files: { 'node_modules/nm-e/package.json': '{"exports":{"./a":"./a.yml"}}', 'node_modules/nm-e/a.yml': 'ignore: [a]\n' } }),
    y('node-module-exports-star', 'extends: nm-e/x/b.yml\n', { files: { 'node_modules/nm-e/package.json': '{"exports":{"./x/*":"./y/*"}}', 'node_modules/nm-e/y/b.yml': 'ignore: [star]\n' } }),
    y('node-module-exports-not-exported', 'extends: nm-e/b\n', { files: { 'node_modules/nm-e/package.json': '{"exports":{"./a":"./a.yml"}}', 'node_modules/nm-e/b.js': '' } }),
    y('node-module-exports-no-main', 'extends: nm-e\n', { files: { 'node_modules/nm-e/package.json': '{"exports":{"./a":"./a.yml"}}' } }),
    y('node-module-exports-missing-target', 'extends: nm-e\n', { files: { 'node_modules/nm-e/package.json': '{"exports":"./gone.yml"}' } }),
    y('node-module-index', 'extends: nm-i\n', { files: { 'node_modules/nm-i/index.json': '{"ignore":["idx"]}' } }),
    y('node-module-js-ext', 'extends: nm-f/a\n', { files: { 'node_modules/nm-f/a.js': 'ignore: [js]\n' } }),
    c('node-module-up', { 'deep/er/.theme-check.yml': 'extends: nm-up/x.yml\n', 'node_modules/nm-up/x.yml': 'ignore: [up]\n' }, { rootRel: 'deep/er' }),
    y('node-module-symlink', 'extends: nm-l/x.yml\n', { files: { 'real/x.yml': 'ignore: [link]\n', 'node_modules/nm-l': { symlink: '../real' } } }),
    y('node-module-relative-inside', 'extends: nm-r/x.yml\n', { files: { 'node_modules/nm-r/x.yml': 'extends: ./y.yml\n', 'node_modules/nm-r/y.yml': 'ignore: [rel]\n' } }),
  ];
  out.push(c('config-path-abs', { 'cfg/c.yml': 'ignore: [viaabs]\nextends: ./d.yml\n', 'cfg/d.yml': 'ignore: [d]\n' }, { config: 'cfg/c.yml' }));
  out.push(c('config-path-beats-default', { [CFG]: 'ignore: [no]\n', 'c.yml': 'ignore: [yes]\n' }, { config: 'c.yml' }));
  out.push(c('config-path-unnormalized', { 'cfg/c.yml': 'extends: ./c.yml\n' }, { config: 'cfg/../cfg/c.yml' }));
  for (const id of ['nothing', 'recommended', 'theme-app-extension', 'all', 'foo']) {
    out.push(c(`identifier-${id}`, { 'x.extension.toml': '', [CFG]: 'ignore: [no]\n' }, { identifier: `theme-check:${id}` }));
  }
  return out;
}

function randomCases(reg, count) {
  const r = rng(0xc0f1);
  const pick = (a) => a[r(a.length)];
  const withSchema = reg.filter((k) => Object.keys(k.schema).length);
  const scalar = () => pick(['1', '0', '-3', '2.5', 'abc', '"q r"', '~', 'true', 'false', '[]', '{}', '[a, b]', '{k: v}', '[1, 2]', "''"]);
  const checkBlock = () => {
    const k = pick([...reg.map((c) => c.code), ...reg.flatMap((c) => c.aliases), 'Unknown', 'other_check']);
    const lines = [`${k}:`];
    const n = r(4);
    for (let i = 0; i < n; i++) {
      const w = r(8);
      if (w === 0) lines.push(`  enabled: ${pick(['true', 'false', '0', '1', '"x"', '~'])}`);
      else if (w === 1) lines.push(`  severity: ${pick(['0', '1', '2', '3', 'error', 'warning', 'info', 'suggestion', 'style', 'bad', '~', '1.5', '[a]'])}`);
      else if (w === 2) lines.push(`  ignore: ${pick(['[a/b]', '["*.x", "y/*"]', 'a', '1', '~', '[1]', '[]'])}`);
      else if (w === 3) {
        const s = pick(withSchema);
        const key = pick(Object.keys(s.schema));
        lines[0] = `${s.code}:`;
        lines.push(`  ${key}: ${scalar()}`);
      } else if (w === 4) {
        const s = pick(withSchema);
        const key = pick(Object.keys(s.schema)).replace(/[A-Z]/g, (m) => `_${m.toLowerCase()}`);
        lines[0] = `${s.code}:`;
        lines.push(`  ${key}: ${scalar()}`);
      } else lines.push(`  ${pick(['foo', 'bar_baz', 'qux_', 'nested'])}: ${scalar()}`);
    }
    return lines.join('\n') + (lines.length === 1 ? ' {}' : '') + '\n';
  };
  const out = [];
  for (let i = 0; i < count; i++) {
    const nfiles = 1 + r(4);
    const names = [CFG, 'a.yml', 'b.yml', 'sub/c.yml'].slice(0, nfiles);
    const files = {};
    names.forEach((name, fi) => {
      let text = '';
      const dir = path.dirname(name) === '.' ? '.' : 'sub';
      const relTo = (t) => (dir === 'sub' ? (t.startsWith('sub/') ? './' + t.slice(4) : '../' + t) : './' + t);
      const forward = names.slice(fi + 1);
      const ext = [];
      // Only one failing target per file: Promise.all's race is out of scope here.
      if (forward.length && r(3)) ext.push(relTo(pick(forward)));
      if (r(4) === 0) ext.push(pick(['nothing', ':nothing', 'theme-check:all', 'recommended', 'theme_app_extension', ':bogus']));
      if (r(40) === 0) ext.length = 0, ext.push('./does-not-exist.yml');
      if (r(30) === 0) ext.length = 0, ext.push(relTo(pick(names)));
      if (ext.length) text += `extends: ${r(2) ? JSON.stringify(ext) : ext.length === 1 ? ext[0] : JSON.stringify(ext)}\n`;
      if (r(4) === 0) text += `context: ${pick(['theme', 'app', 'other'])}\n`;
      if (r(4) === 0) text += `root: ${pick(['dist', './x/y', '..', 'a b', '/abs', '1'])}\n`;
      if (r(3) === 0) text += `ignore: ${pick(['[a]', 'b', '["c/*", "/d"]', '[]', '1'])}\n`;
      if (r(8) === 0) text += 'include_categories: [x]\n';
      const nc = r(4);
      for (let k = 0; k < nc; k++) text += checkBlock();
      files[name] = text;
    });
    // A later duplicate key is a YAML error in both; keep those, they are valid cases.
    out.push({ name: `random-${i}`, files });
  }
  return out;
}

function yamlValueCases() {
  const r = rng(0x5eed);
  const pick = (a) => a[r(a.length)];
  const strPool = [
    '', ' ', 'a', 'a b', 'true', 'True', 'null', '~', '1', '1.5', '0x10', '0o7', '1e3', '.inf', '.nan', '-', '?', ':', '- a', '? a', 'a: b', 'a:', ':a', '#c', 'a #c', 'a#c',
    "it's", 'say "hi"', `both ' and "`, '*a', '&a', '!a', '|', '>', '%x', '@x', '`x', ',a', '[a', '{a', 'a]', 'a,b', '---', '...', '--- a', 'a\nb', 'a\n\nb', 'a\n', 'a\n\n', '\na', ' a', 'a ', '\ta', 'a\t',
    'a\n b', 'a \nb', 'tab\there', 'é', 'ünï', '日本語', '😀', '\u0000', '\u0007', '\u001b', '\u0085', '\u00a0', '\u2028', '\u007f', 'x\u0001y', 'node_modules/**', '*.foo', 'file:///a/b', 'file:///a b', '[a, b]',
    'word '.repeat(30).trim(), 'x'.repeat(100), 'long line with "quote" '.repeat(8).trim(), "long 'single' line ".repeat(8).trim(), 'a: b '.repeat(30), '#'.repeat(90), ('w'.repeat(30) + ' ').repeat(5).trim(), 'multi\nline '.repeat(12), 'line one\nline two is long '.repeat(6) + '\n', 'é '.repeat(60), '\u0001 '.repeat(60), ('y'.repeat(79) + ' z').repeat(2), '  leading spaces and a long tail '.repeat(4), 'trailing newlines\n\n\n', 'a\n  more\n    indented\nb', '\n\nstart', ' \n', '\n ', 'x\n ', '%a\nb', 'a\n---\nb', 'a\n...\n', 'key: "v"\nother',
  ];
  const keyPool = ['a', 'b c', 'true', 'null', '1', '', 'x'.repeat(1030), 'multi\nline', 'undefined', '- k', 'é', '---', '%k', "q'k", 'q"k', 'a: b', '#k', 'long '.repeat(30)];
  const val = (d) => {
    const w = r(d > 3 ? 7 : 11);
    switch (w) {
      case 0: return null;
      case 1: return pick([true, false]);
      case 2: return pick([0, 1, -1, 1.5, 1e21, 1e-7, 123456789, -0.5, 1e300]);
      case 3: case 4: case 5: case 6: return pick(strPool);
      case 7: case 8: return Array.from({ length: r(4) }, () => val(d + 1));
      default: return Object.fromEntries(Array.from({ length: r(4) }, () => [pick(keyPool), val(d + 1)]));
    }
  };
  const out = [];
  for (const s of strPool) { out.push(s); out.push({ k: s }); out.push([s]); out.push({ a: { b: [s] } }); out.push({ [s]: 1 }); out.push([[s]]); }
  for (let i = 0; i < 6000; i++) out.push(val(0));
  return out;
}

function yamlParseCases() {
  const seeds = [
    'a: 1\nb: 2\n', 'a:\n  - x\n  - y\n', '- a\n- b: 1\n  c: 2\n- - n\n  - m\n', 'a: [1, 2, {b: c}]\n', '{a: 1, b: [x, y], "c": d}\n', 'a: {b: {c: [d]}}\n',
    "s: 'it''s'\nd: \"q\\n\\t\\x41\\u00e9\\U0001F600\\\"\"\n", 'p: a b\n  c d\n\n  e\nq: x\n', "q: 'a\n  b\n\n  c'\nd: \"a\n  b\\\n  c\n\n  d\"\n",
    'a: |\n  x\n   y\n\n  z\nb: |-\n  x\nc: |+\n  x\n\nd: >\n  x\n  y\n\n  z\n   m\n  n\ne: >-\n  q\nf: >+\n  q\n\n', 'a: |2\n    x\nb: >1\n  y\n', '--- |\nx\n', '--- >\n  a\n  b\n...\n',
    'a: &x 1\nb: *x\nc: &y [*x, 2]\nd: *y\n', '? a\n: b\n? [c]\n: d\n? e\n', '? |\n  block\n: v\n', 'a: ~\nb: null\nc: Null\nd: NULL\ne:\nf: true\ng: True\nh: TRUE\ni: false\nj: False\nk: FALSE\n',
    'a: 1\nb: -1\nc: +1\nd: 0x1F\ne: 0o17\nf: 1.5\ng: -.5\nh: 1.\ni: 1e3\nj: 1E-3\nk: .inf\nl: -.INF\nm: .nan\nn: .NaN\no: 0123\np: 1_000\nq: 0x\nr: 0b1\ns: 1e\nt: +.inf\n',
    '# c\na: 1 # c\n# d\nb: # e\n  - x # f\n  # g\n  - y\n# end\n', '%YAML 1.2\n---\na: 1\n...\n', '%TAG !e! tag:x:\n---\na: !e!t 1\n', '---\na: 1\n---\nb: 2\n', '---\n...\n---\n', 'a: 1\n...\nb: 2\n',
    '!!str 1\n', 'a: !!str 1\nb: !!int "2"\nc: !!float 3\nd: !!bool true\ne: !!null ~\nf: !foo x\ng: !!map {a: 1}\nh: !!seq [1]\ni: ! 5\nj: !<tag:x> y\n', '&a a: &b b\n*a : c\n', '"a": 1\n\'b\': 2\n[c]: 3\n{d: e}: 4\n', 'a: [\n  1,\n  2,\n]\nb: {\n  c: d\n}\n',
    '[a, , b]\n', '{a, b: }\n', '[a: 1, b]\n', '[? a : b]\n', '{? a : b, ? c}\n', 'a: b: c\n', 'a:\n\tb: 1\n', '\ta: 1\n', 'a\tb: 1\n', '- a\n  - b\n', '- a\n - b\n', 'a: 1\n b: 2\n', ' a: 1\nb: 2\n', 'a: - b\n', '- - - a\n', '-\n-\n', ':\n', ': a\n', '?\n', 'a: "x\n', "a: 'x\n", 'a: [x\n', 'a: {x\n', ']\n', '}\n', ',\n', 'a: ]\n', 'a: }\n', 'a: *\n', 'a: &\n', '&a\n', '*a\n', '*a : 1\n', 'a: &x &y 1\n', 'a: !t !u 1\n', '@a\n', '`a\n', '%a\n', 'a: @b\n', 'a: `b\n', '|\n', '>\n', '|x\n', '| # c\n  a\n', '|1 2\n', 'a: |\nb: 2\n', 'a: |\n x\ny\n', 'a: |\n  x\n y\n',
    'a: "\\q"\n', 'a: "\\xZZ"\n', 'a: "\\uD83D"\n', 'a: "\\U00110000"\n', 'x'.repeat(1030) + ': 1\n', 'a#b: 1\n', 'a #b: 1\n', '"a"#b\n', '"a" #b\n', "'a'b\n", '"a"b\n', '[a]b\n', '{a}b\n', 'a: [b]c\n', 'é: "\\q"\n', '日本語: [\n', 'a: 😀 *\n😀😀: "\\q"\n', 'a: 1\r\nb: 2\r\n', 'a: |\r\n  x\r\n  y\r\n', 'a: "x\r\n  y"\r\n', '\ufeffa: 1\n', '\ufeff\ufeffa: 1\n', '- \ufeffa\n', '---a\n', '--- a\n', '...a\n', '.a\n', '-a\n', '?a: 1\n', ':a: 1\n', 'a:b: 1\n', 'http://x: 1\n', '[a:b, c: d, e:]\n', '{a:b, c: d, "e":f}\n', 'a: b\n---\n', '---\n---\n', '# only\n---\n', '---\n# only\n', '--- # c\na: 1\n', '--- a: 1\n', '--- - a\n', '--- [a]\n', '--- {a: 1}\n', '--- &x\n', '--- !!str\n', '&x\n---\n', '!!str\n---\n',
  ];
  const r = rng(0xfa11);
  const out = [...seeds];
  const chars = [' ', '\n', ':', '-', '#', '[', ']', '{', '}', ',', '"', "'", '&', '*', '!', '|', '>', '?', '\t', 'x', '%', '@', '`', '.', '\\', 'é', '\r\n', '  '];
  for (let i = 0; i < 40000; i++) {
    let s = seeds[r(seeds.length)];
    const edits = 1 + r(3);
    for (let e = 0; e < edits; e++) {
      const at = r(s.length + 1);
      switch (r(4)) {
        case 0: s = s.slice(0, at) + s.slice(at + 1 + r(3)); break;
        case 1: s = s.slice(0, at) + chars[r(chars.length)] + s.slice(at); break;
        case 2: s = s.slice(0, at) + chars[r(chars.length)] + s.slice(at + 1); break;
        default: { const b = seeds[r(seeds.length)]; s = s.slice(0, at) + b.slice(0, r(b.length)); }
      }
    }
    if (!/[\ud800-\udfff]/.test(s) || !/[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/.test(s)) out.push(s);
  }
  return out;
}

// `lone-surrogate`: a `\\uD83D` escape, which a Rust string cannot hold.
// `collection-key`: a flow collection as a map key; upstream prints its source
// style (quotes, anchors, empty values), lq stringifies the resolved value.
function namedYamlDeviation(expected) {
  if (/\\ud[89ab][0-9a-f]{2}(?!\\ud[c-f])|(?<!\\ud[89ab][0-9a-f]{2})\\ud[c-f][0-9a-f]{2}/.test(expected)) return 'lone-surrogate';
  if (/"[[{] /.test(expected) || /[{,]"[[{]/.test(expected)) return 'collection-key';
  return null;
}

function runYamlHarness(log) {
  const parseCases = yamlParseCases();
  const valueCases = yamlValueCases();
  const file = path.join(GEN, 'yaml-cases.ndjson');
  const fd = fs.openSync(file, 'w');
  const expected = [];
  let surrogates = 0;
  for (const src of parseCases) {
    fs.writeSync(fd, JSON.stringify({ src }) + '\n');
    let e;
    try {
      e = JSON.stringify({ value: YAML.parse(src) });
    } catch (err) {
      e = JSON.stringify({ error: { name: err.name, message: err.message } });
    }
    expected.push(e);
  }
  for (const value of valueCases) {
    fs.writeSync(fd, JSON.stringify({ value }) + '\n');
    expected.push(JSON.stringify({ str: YAML.stringify(value) }));
  }
  fs.closeSync(fd);
  const stdout = execFileSync(BIN, ['yaml', '--cases', file], { encoding: 'utf8', maxBuffer: 1 << 30 });
  const lines = stdout.split('\n');
  const yd = path.join(GEN, 'yaml-diffs.txt');
  const dd = fs.openSync(yd, 'w');
  const counts = { parse: [0, 0], parseErr: [0, 0], stringify: [0, 0] };
  const unmatched = {};
  const cases = [...parseCases.map((src) => ({ src })), ...valueCases.map((value) => ({ value }))];
  cases.forEach((c, i) => {
    const kind = c.value !== undefined || 'value' in c ? 'stringify' : expected[i].startsWith('{"error"') ? 'parseErr' : 'parse';
    counts[kind][0]++;
    if (expected[i] !== lines[i]) {
      // Named deviations: both are documented at the top of yaml.rs.
      const named = namedYamlDeviation(expected[i]);
      if (named) {
        unmatched[named] = (unmatched[named] ?? 0) + 1;
        fs.writeSync(dd, `[${named}] ${JSON.stringify(c)}\n  oracle ${expected[i]}\n  lq     ${lines[i]}\n`);
        return;
      }
      counts[kind][1]++;
      fs.writeSync(dd, `${JSON.stringify(c)}\n  oracle ${expected[i]}\n  lq     ${lines[i]}\n`);
    }
  });
  fs.closeSync(dd);
  const total = counts.parse[0] + counts.parseErr[0] + counts.stringify[0];
  const diffs = counts.parse[1] + counts.parseErr[1] + counts.stringify[1];
  log(`[yaml] cases=${total} diffs=${diffs} (parse ${counts.parse[0]}/${counts.parse[1]}, errors ${counts.parseErr[0]}/${counts.parseErr[1]}, stringify ${counts.stringify[0]}/${counts.stringify[1]})`);
  for (const [k, v] of Object.entries(unmatched)) log(`[yaml] unmatched ${k}=${v}`);
  return surrogates;
}

async function main() {
  const reg = writeRegistry();
  fs.rmSync(WORK, { recursive: true, force: true });
  fs.mkdirSync(WORK, { recursive: true });
  const d = fs.openSync(DIFFS, 'w');
  let total = 0;
  let diffs = 0;
  const compare = async (label, configPath, root) => {
    total++;
    const o = await oracleConfig(configPath, root);
    const l = lqConfig(configPath, root);
    const oe = stderrFilter(o.stderr);
    const le = stderrFilter(l.stderr);
    if (o.out !== l.out || oe !== le) {
      diffs++;
      fs.writeSync(d, `--- ${label} config=${configPath} root=${root}\n  oracle ${o.out.slice(0, 3000)}\n  lq     ${l.out.slice(0, 3000)}\n`);
      if (oe !== le) fs.writeSync(d, `  oracle-stderr ${oe}\n  lq-stderr     ${le}\n`);
    }
  };

  for (const [name, root] of Object.entries(THEMES)) {
    await compare(`theme:${name}`, undefined, root);
    for (const id of ['nothing', 'recommended', 'theme-app-extension', 'all']) {
      await compare(`theme:${name}:${id}`, `theme-check:${id}`, root);
    }
  }
  const legacy = [':default', ':nothing', ':theme_app_extension', ':all', 'recommended', 'all', 'nothing', 'theme_app_extensions'];
  for (const l of legacy) {
    const dir = path.join(WORK, `legacy-${l.replace(/:/g, '_')}`);
    writeTree(dir, { [CFG]: `extends: ${JSON.stringify(l)}\n` });
    await compare(`legacy:${l}`, undefined, dir);
  }
  const cases = [...handCases(reg), ...randomCases(reg, 300)];
  let n = 0;
  for (const c of cases) {
    const dir = path.join(WORK, String(n++).padStart(4, '0'));
    fs.mkdirSync(dir, { recursive: true });
    writeTree(dir, c.files);
    const root = c.rootRel ? path.join(dir, c.rootRel) : dir;
    const config = c.identifier ?? (c.config ? path.join(dir, c.config) : undefined);
    await compare(c.name, config, root);
  }
  fs.closeSync(d);
  console.log(`[config] cases=${total} diffs=${diffs}`);
  if (diffs) console.log(`  details: ${path.relative(ROOT, DIFFS)}`);
  runYamlHarness(console.log);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  await main();
}
