// Differential test of crates/lq-theme's file discovery, isIgnored, minimatch and
// collation against the oracle's glob, theme-check and minimatch packages.
import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import { ROOT, THEMES, GEN, oracleRequire, rng } from './lib.mjs';
import { writeRegistry } from './config.mjs';

process.removeAllListeners('warning');
process.on('warning', () => {});

const PROFILE = process.env.LQ_PROFILE ?? 'fast';
const BIN = path.join(ROOT, 'target', PROFILE, 'lq-theme-debug');
const REGISTRY = path.join(GEN, 'registry.json');
const WORK = path.join(GEN, 'discovery-trees');
const DIFFS = path.join(GEN, 'discovery-diffs.txt');

const common = oracleRequire('@shopify/theme-check-common');
const node = oracleRequire('@shopify/theme-check-node');
const glob = promisify(oracleRequire('glob'));
const { minimatch } = oracleRequire('minimatch');

const diffs = fs.openSync(DIFFS, 'w');
const note = (s) => fs.writeSync(diffs, s + '\n');

async function oracleFiles(root) {
  const config = await node.loadConfig(undefined, root);
  const pattern = node.getThemeFilesPathPattern(config.rootUri);
  const paths = await glob(pattern, { absolute: true });
  return paths.filter((p) => !common.isIgnored(p, config));
}

function lqFiles(root) {
  const r = spawnSync(BIN, ['files', '--root', root, '--registry', REGISTRY], { encoding: 'utf8', maxBuffer: 1 << 28 });
  return r.stdout.split('\n').filter(Boolean).map((l) => {
    const v = JSON.parse(l);
    return v.error ? `THROW ${v.error.name}: ${v.error.message}` : v;
  });
}

function writeTree(dir, files) {
  for (const f of files) {
    const p = path.join(dir, typeof f === 'string' ? f : f.path);
    fs.mkdirSync(path.dirname(p), { recursive: true });
    if (typeof f === 'string') {
      if (f.endsWith('/')) fs.mkdirSync(p, { recursive: true });
      else fs.writeFileSync(p, '');
    } else if (f.link) fs.symlinkSync(f.link, p);
    else fs.writeFileSync(p, f.content ?? '');
  }
}

const TREES = {
  basic: ['a.liquid', 'b.json', 'c.txt', 'sections/x.liquid', 'templates/y.json', 'assets/z.css', 'assets/z.js', 'locales/en.default.json', 'deep/er/still/f.liquid'],
  dots: ['.hidden.liquid', '.hidden.json', '.liquid', '.json', '.dot/a.liquid', '.dot/sub/b.json', 'x/.y/c.liquid', 'x/.z.json', 'x/ok.liquid', '..double.json', 'a.b.c.json', 'a..json', 'no-ext', 'liquid', 'json'],
  caseSensitive: ['UP.LIQUID', 'Up.Json', 'a.Liquid', 'a.JSON', 'A.liquid', 'B.json', 'a.liquid', 'b.json', 'Z/a.json', 'z/b.json', 'Aa/c.json', 'aa/c.json', 'aA/d.json', 'AA/d.json'],
  spaces: ['a b.liquid', ' lead.liquid', 'trail .json', 'dir with space/x y.json', 'tab\there.json', 'new\nline.json', '\nleadnl.json', '\rleadcr.json', 'x y.json', ' lead.json'],
  unicode: ['é.liquid', 'e.liquid', 'z.liquid', 'ü/ñ.json', 'u/n.json', '日本語.liquid', 'Ω.json', 'ß.json', 'ss.json', 'æ.json', 'ae.json', 'ø.json', 'o.json', 'Å.json', 'å.json', 'a.json', 'A.json', 'emoji😀.json', '€.json', 'naïve.json', 'naive.json', 'NAIVE.json', '_u.json', '-u.json', '1u.json'],
  hash: ['#a.liquid', 'b#c.json', '#dir/d.liquid', '%20.json', '%.json', 'q?.json', 'a&b.json', 'a=b.json', 'a+b.json', 'a,b.json', "a'b.json", 'a"b.json', 'a;b.json', 'a:b.json', 'a@b.json', 'a~b.json', 'a!b.json', 'a$b.json', 'a(b).json', 'a[b].json', 'a{b}.json', 'a*b.json', 'a|b.json', 'a<b>.json', 'a^b.json', 'a`b.json', 'a\\b.json'],
  punctuationOrder: ['a-b.json', 'a_b.json', 'a.b.json', 'a b.json', 'a,b.json', 'a;b.json', 'a:b.json', 'a!b.json', 'a?b.json', "a'b.json", 'a"b.json', 'a(b.json', 'a)b.json', 'a[b.json', 'a]b.json', 'a{b.json', 'a}b.json', 'a@b.json', 'a*b.json', 'a&b.json', 'a#b.json', 'a%b.json', 'a`b.json', 'a^b.json', 'a+b.json', 'a<b.json', 'a=b.json', 'a>b.json', 'a|b.json', 'a~b.json', 'a$b.json', 'a0b.json', 'a9b.json', 'aab.json', 'aAb.json', 'azb.json', 'aZb.json', 'ab.json', 'a.json', 'a/b.json', 'a-/b.json', 'a_/b.json', 'a1/b.json', 'A/b.json', 'A/B.json', 'a/B.json', 'a10.json', 'a2.json', 'a02.json', 'b.json', 'B.json', '0.json', '10.json', '2.json', '_.json', '-.json', '..json', '~.json'],
  symlinks: ['real/a.liquid', 'real/sub/b.json', 'real/sub/deep/c.json', 'f.txt', 'target.liquid', { path: 'dirlink', link: 'real' }, { path: 'filelink.liquid', link: 'target.liquid' }, { path: 'brokenlink.json', link: 'missing' }, { path: 'textlink.json', link: 'f.txt' }, { path: 'dirlink.json', link: 'real' }, { path: 'nested/uplink', link: '..' }, { path: 'nested/self', link: 'self' }, { path: '.hiddenlink', link: 'real' }, { path: 'real/sub/innerlink', link: '../../real' }],
  symlinkOutside: ['in/a.liquid', { path: 'out', link: '../basic' }, { path: 'in/out2', link: '../../dots' }],
  dirNamedLikeFile: ['d.json/inner.liquid', 'd.liquid/inner.json', 'd.liquid/more/x.json', 'e.json/'],
  empty: [],
  ignoreFiles: [
    { path: '.theme-check.yml', content: "ignore:\n  - node_modules/**\n  - snippets/*\n  - '*.json'\n  - /rooted/**\n  - sub/**/*.liquid\n  - '**/deep.liquid'\n" },
    'a.liquid', 'a.json', 'snippets/x.liquid', 'snippets/n/y.liquid', 'node_modules/p/x.liquid', 'rooted/k.liquid', 'sub/a/b.liquid', 'sub/c.liquid', 'x/deep.liquid', 'keep/k.liquid',
  ],
  prune: [
    { path: '.theme-check.yml', content: "ignore:\n  - node_modules/**\n  - a/*\n  - b/**/c/**\n  - '**/d/**'\n  - '*.x/**'\n  - e/f.json/**\n" },
    'node_modules/p/x.json', 'node_modules/.d/y.json', 'q/node_modules/x.liquid', 'a/1.json', 'a/n/2.json', 'b/c/3.json', 'b/z/c/4.json', 'b/c.json', 'b/z.json', 'r/d/5.json', 'd/s/6.liquid', 'k.x/7.json', 'k.y/8.json', 'e/f.json/9.json', 'e/g.json', 'keep.json', 'j/k/l.liquid', { path: 'lnk', link: 'node_modules/p' }, { path: 'lnk2', link: 'j' }, { path: 'a.json', link: 'keep.json' },
  ],
  '.dotroot/inner': [
    { path: '.theme-check.yml', content: 'ignore:\n  - node_modules/**\n  - x/*\n' },
    'node_modules/p/x.json', 'x/1.json', 'y/2.json',
  ],
  rootSetting: [{ path: '.theme-check.yml', content: 'root: dist\n' }, 'top.liquid', 'dist/in.liquid', 'dist/sub/in.json'],
  rootSettingEscape: [{ path: '.theme-check.yml', content: 'root: ../basic\n' }],
};

const MAGIC_ROOTS = {
  'with space': ['a.liquid', 's/b.json'],
  'paren(s)': ['a.liquid', 's/b.json'],
  'hash#dir': ['a.liquid', 's/b.json'],
  'percent%20dir': ['a.liquid', 's/b.json'],
  'q?mark': ['a.liquid', 's/b.json'],
  'br[ack]et': ['a.liquid', 's/b.json'],
  'brack': ['c.liquid'],
  'brace{a,b}': ['a.liquid'],
  'bracea': ['d.liquid'],
  'braceb': ['e.liquid'],
  'st*r': ['a.liquid'],
  'star': ['f.liquid'],
  'ext@(x)': ['a.liquid'],
  'extx': ['g.liquid'],
  '!bang': ['a.liquid'],
  'dollar$&x': ['a.liquid'],
  'uni-é': ['a.liquid'],
};

async function runDiscovery(log) {
  let total = 0;
  let bad = 0;
  const check = async (label, root) => {
    total++;
    let o;
    try {
      o = await oracleFiles(root);
    } catch (e) {
      o = [`THROW ${e.name}: ${e.message}`];
    }
    const l = lqFiles(root);
    if (JSON.stringify(o) !== JSON.stringify(l)) {
      bad++;
      note(`--- discovery ${label}`);
      const os = new Set(o);
      const ls = new Set(l);
      note(`  only-oracle ${JSON.stringify(o.filter((x) => !ls.has(x))).slice(0, 1500)}`);
      note(`  only-lq     ${JSON.stringify(l.filter((x) => !os.has(x))).slice(0, 1500)}`);
      for (let i = 0; i < Math.min(o.length, l.length); i++) {
        if (o[i] !== l[i]) {
          note(`  first-order-diff at ${i}: oracle=${JSON.stringify(o[i])} lq=${JSON.stringify(l[i])}`);
          break;
        }
      }
    }
  };
  for (const [name, root] of Object.entries(THEMES)) await check(`theme:${name}`, root);
  fs.rmSync(WORK, { recursive: true, force: true });
  fs.mkdirSync(WORK, { recursive: true });
  for (const [name, files] of Object.entries(TREES)) {
    const dir = path.join(WORK, name);
    fs.mkdirSync(dir, { recursive: true });
    writeTree(dir, files);
  }
  for (const name of Object.keys(TREES)) await check(`tree:${name}`, path.join(WORK, name));
  await check('symlinked-root', (() => {
    const p = path.join(WORK, 'rootlink');
    fs.symlinkSync('basic', p);
    return p;
  })());
  await check('missing-root-dir-setting', (() => {
    const p = path.join(WORK, 'gone-root');
    writeTree(p, [{ path: '.theme-check.yml', content: 'root: does-not-exist\n' }]);
    return p;
  })());
  // Generated trees: names mixing case and punctuation, ordered by real glob.
  const r = rng(0xd15c);
  const nameChars = ['a', 'A', 'b', 'B', 'z', 'Z', '0', '1', '9', '-', '_', '.', ' ', '#', '+', '~', ',', '(', ')', '[', 'é', 'É', 'ß'];
  for (let t = 0; t < 40; t++) {
    const dir = path.join(WORK, `gen${t}`);
    const files = [];
    const count = 5 + r(40);
    const seg = () => Array.from({ length: 1 + r(5) }, () => nameChars[r(nameChars.length)]).join('');
    for (let i = 0; i < count; i++) {
      const depth = r(3);
      const parts = Array.from({ length: depth }, seg);
      const ext = ['.liquid', '.json', '.liquid', '.json', '.txt'][r(5)];
      files.push([...parts, seg() + ext].join('/'));
    }
    fs.mkdirSync(dir, { recursive: true });
    for (const f of files) {
      try {
        writeTree(dir, [f]);
      } catch {
        // A path clashing with an earlier file: drop it.
      }
    }
    await check(`gen:${t}`, dir);
  }
  const magicBase = path.join(WORK, 'magic');
  for (const [name, files] of Object.entries(MAGIC_ROOTS)) {
    const dir = path.join(magicBase, name);
    fs.mkdirSync(dir, { recursive: true });
    writeTree(dir, files);
  }
  for (const name of Object.keys(MAGIC_ROOTS)) await check(`magic-root:${name}`, path.join(magicBase, name));
  log(`[discovery] cases=${total} diffs=${bad}`);
}

// ---- minimatch -------------------------------------------------------------

function genMinimatchCases(n) {
  const r = rng(0x6d6d);
  const pick = (a) => a[r(a.length)];
  const lit = ['a', 'b', 'c', 'foo', 'bar', 'x.js', 'y.css', 'index.liquid', 'a-b', 'a_b', 'a b', 'é', 'Z', '1', '.', '..', '.a', '.foo', '.git', 'a.b.c', '+', '@', '!', '(', ')', '{', '}', '[', ']', '|', ',', '#x', '$', '^', 'a\\b', '%', '~'];
  const segs = [
    ...lit,
    '*', '**', '***', '*.js', '*.*', '.*', '?', '??', '?.js', 'a*', '*a', 'a*b', 'a?', '?a', '*a*', '**a', 'a**', '**.js', '.**', '.?', '.*.js', '*.', '*.j?',
    '[abc]', '[a-c]', '[!a]', '[^a]', '[a-c]*', '*[a-c]', '[.]a', '[a]', '[*]', '[]]', '[!]]', '[]', '[!]', '[a', 'a]', '[z-a]', '[a-a]', '[a-]', '[-a]', '[\\]]', '[\\a]', '[\\\\]', '[[]', '[]-a]',
    '[[:alpha:]]', '[[:digit:]]', '[[:alnum:]]', '[[:upper:]]', '[[:lower:]]', '[[:space:]]', '[[:punct:]]', '[[:word:]]', '[[:xdigit:]]', '[[:ascii:]]', '[[:cntrl:]]', '[[:print:]]', '[[:graph:]]', '[[:blank:]]', '[![:alpha:]]', '[[:alpha:]0-9]', '[a-[:alpha:]]', '[[:foo:]]', '[[:alpha:]],x', '[[:alpha:]]-x', '[[:alpha:]]x', '[[:alpha:]]#', '[[:alpha:]] ',
    '\\*', '\\?', '\\a', '\\[a\\]', 'a\\', '\\\\', '\\.', '\\!', '\\(', '\\|', '\\,', '\\-', '\\{',
    '@(a|b)', '@(a)', '@()', '@(*)', '@(a|)', '@(|a)', '?(a|b)', '?(a)', '?()', '+(a|b)', '+(a)', '+()', '+(*)', '*(a|b)', '*(a)', '*()', '*(*)', '!(a|b)', '!(a)', '!()', '!(*)', '!(.*)', '!(*.js)', 'x!(a)', '!(a)x', 'x!(a)y', '!(a)!(b)', 'a@(b|c)d', '*.@(js|css)', '*.!(js|css)', '@(a|b)*', '*(.a)', '+(.a)', '@(.a|b)', '?(.a)', '!(.a)', '@(a', '!(a', '+(a', '*(a', '?(a', '@(a|b', 'a)', '|', '(a)', '(a|b)',
    '@(@(a|b)|c)', '@(a|@(b|c))', '*(*(a))', '*(+(a))', '*(?(a))', '*(@(a))', '+(+(a))', '+(@(a))', '+(?(a))', '+(*(a))', '?(?(a))', '?(@(a))', '?(*(a))', '?(+(a))', '@(?(a))', '@(*(a))', '@(+(a))', '@(!(a))', '!(!(a))', '!(@(a))', '!(?(a))', '!(*(a))', '!(+(a))', '!(a|!(b))', '@(a|?(b))', '+(a|?(b))', '!(a|?(b))', '@(@(@(@(a))))', '!(!(!(!(a))))', '+(a|+(b|+(c|+(d))))', '@(x|[a-c])', '@([a-c]|x)', '+([a-c])', '!([a-c])', '@(a\\|b)', '@(a\\)b)', '@([)]|a)', '@([|]|a)', '!(a)@(b)', '@(a)!(b)', '@(a)*', '*@(a)', '?@(a)', '[a]@(b)', '!(a|b)*', '*!(a|b)', '!(a)*!(b)', '@(!(a)|b)', '!(@(a|b)c)', '!(x)y!(z)',
    '{a,b}', '{a,}', '{,a}', '{a}', '{}', '{,}', '{a,b}c', 'c{a,b}', '{a,{b,c}}', '{{a,b},c}', '{1..3}', '{01..03}', '{a..c}', '{c..a}', '{1..5..2}', '{a..e..2}', '{1..}', '{..3}', '{a,b}{c,d}', '{a,b', 'a,b}', '${a,b}', '\\{a,b\\}', '{a\\,b,c}', '{a},b}', '{{a}', '{a}}', '{-1..1}', '{-01..1}', '{Z..a}', '{*,?}', '{*.js,*.css}', '{a,b}/c', 'a/{b,c}', '{a/b,c}', '{a,b/**}',
  ];
  const pathSeg = () => {
    const w = r(10);
    if (w < 7) return pick(lit);
    if (w === 7) return pick(['abc', 'abd', 'foo.js', 'bar.css', 'x.y.z', 'aab', 'abb', 'ccc', '.foo.js', 'ab', 'ba', 'aa', 'bb', '1', '12', 'A', 'B', 'AZ', 'a1', '_', '-', ' ', 'c.js', 'a.', 'é.js']);
    if (w === 8) return '';
    return pick(segs).replace(/[*?]/g, 'a');
  };
  const makePath = () => {
    const n = r(5);
    let parts = Array.from({ length: n + (r(4) ? 1 : 0) }, pathSeg);
    let s = parts.join('/');
    const w = r(12);
    if (w === 0) s = '/' + s;
    if (w === 1) s += '/';
    if (w === 2) s = s.replace('/', '//');
    if (w === 3) s = 'file:///root/' + s;
    if (w === 4) s = './' + s;
    return s;
  };
  const makePattern = () => {
    const k = 1 + r(4);
    let s = Array.from({ length: k }, () => pick(segs)).join('/');
    const w = r(16);
    if (w === 0) s = '/' + s;
    if (w === 1) s += '/';
    if (w === 2) s = '!' + s;
    if (w === 3) s = '!!' + s;
    if (w === 4) s = '#' + s;
    if (w === 5) s = s.replace('/', '//');
    if (w === 6) s = '**/' + s;
    if (w === 7) s += '/**';
    if (w === 8) s = 'file:///root/' + s;
    if (w === 9) s = './' + s;
    if (w === 10) s += '/..';
    return s;
  };
  // A path derived from the pattern hits far more interesting branches.
  const fromPattern = (p) => {
    const parts = p.replace(/^[!#]+/, '').split('/');
    return parts
      .map((x) => {
        if (x === '**') return Array.from({ length: r(3) }, pathSeg).join('/');
        return x
          .replace(/\{([^{}]*)\}/g, (_, b) => pick(b.split(',')) ?? '')
          .replace(/[@?+*!]\(([^()]*)\)/g, (_, b) => pick(b.split('|')) ?? '')
          .replace(/\[!?\^?([^\]]*)\]/g, (_, b) => b[0] ?? '')
          .replace(/\*+/g, () => pick(['', 'a', 'xy', '.z']))
          .replace(/\?/g, () => pick(['a', 'b', '.']))
          .replace(/\\/g, '');
      })
      .join('/');
  };
  const out = [];
  const seen = new Set();
  for (let i = 0; seen.size < n; i++) {
    const pattern = i % 3 === 2 ? pick(segs) : makePattern();
    for (let k = 0; k < 3; k++) {
      const p = k === 0 ? fromPattern(pattern) : makePath();
      const key = `${p}\0${pattern}`;
      if (!seen.has(key)) {
        seen.add(key);
        out.push({ path: p, pattern });
      }
    }
  }
  return out;
}

function runMinimatch(log) {
  const cases = genMinimatchCases(60000);
  const file = path.join(GEN, 'minimatch-cases.ndjson');
  fs.writeFileSync(file, cases.map((c) => JSON.stringify(c)).join('\n') + '\n');
  const expected = cases.map((c) => {
    try {
      return minimatch(c.path, c.pattern) ? '1' : '0';
    } catch {
      return '!';
    }
  });
  const got = execFileSync(BIN, ['minimatch', '--cases', file], { encoding: 'utf8', maxBuffer: 1 << 28 }).split('\n');
  let bad = 0;
  let throws = 0;
  let hits = 0;
  cases.forEach((c, i) => {
    if (expected[i] === '!') throws++;
    if (expected[i] === '1') hits++;
    if (expected[i] !== got[i]) {
      bad++;
      if (bad <= 400) note(`--- minimatch ${JSON.stringify(c)} oracle=${expected[i]} lq=${got[i]}`);
    }
  });
  log(`[minimatch] cases=${cases.length} diffs=${bad} (matches=${hits} throws=${throws})`);
}

function runIgnored(log) {
  const r = rng(0x1661);
  const pick = (a) => a[r(a.length)];
  const roots = ['file:///root', 'file:///root/sub dir', 'file:///r$&x', 'file:///r$1', 'file:///r$$', "file:///r$'", 'file:///'];
  const patterns = ['a', 'a/*', 'a/**', '/a', '/a/*', '/a/**', '*.js', '/*.js', '**/x', 'a/b.js', '/a/b.js', 'assets/*.js', '/assets/*.js', '', '/', '//', '*', '/*', 'a/', '!a', '#a', 'x/*/*', 'x/*/y/*', '/**', '**', '{a,b}/*', '/{a,b}/c', '*.{js,css}', 'node_modules/**', '.git/*', '.*', '**/.*', '@(a|b)/*', '/x/*'];
  const uris = ['a', 'a/b', 'a/b.js', 'x/a/b.js', 'x/y', 'x/k/y/z', 'b/c', 'assets/m.js', 'q/assets/m.js', 'n.js', 'deep/er/n.js', 'node_modules/p/i.js', '.git/c', 'd/.git/c', '.h', 'd/.h', 'c.css', 'z', ''];
  const cases = [];
  for (let i = 0; i < 20000; i++) {
    const rootUri = pick(roots);
    const rel = pick(uris);
    const base = rootUri.replace(/\/$/, '');
    const uri = pick([`${base}/${rel}`, `${base}/${rel}`, `/abs${base.slice(7)}/${rel}`, rel]);
    const c = { uri, rootUri, ignore: Array.from({ length: r(3) }, () => pick(patterns)) };
    if (r(2)) c.checkIgnore = Array.from({ length: r(3) }, () => pick(patterns));
    cases.push(c);
  }
  const file = path.join(GEN, 'ignored-cases.ndjson');
  fs.writeFileSync(file, cases.map((c) => JSON.stringify(c)).join('\n') + '\n');
  const def = { meta: { code: 'Check' } };
  const expected = cases.map((c) => {
    const config = { rootUri: c.rootUri, ignore: c.ignore, settings: c.checkIgnore ? { Check: { ignore: c.checkIgnore } } : {} };
    return common.isIgnored(c.uri, config, c.checkIgnore ? def : undefined) ? '1' : '0';
  });
  const got = execFileSync(BIN, ['ignored', '--cases', file], { encoding: 'utf8', maxBuffer: 1 << 28 }).split('\n');
  let bad = 0;
  let hits = 0;
  cases.forEach((c, i) => {
    if (expected[i] === '1') hits++;
    if (expected[i] !== got[i]) {
      bad++;
      if (bad <= 200) note(`--- ignored ${JSON.stringify(c)} oracle=${expected[i]} lq=${got[i]}`);
    }
  });
  log(`[ignored] cases=${cases.length} diffs=${bad} (matches=${hits})`);
}

function runSort(log) {
  const r = rng(0x5047);
  const latin = [];
  for (let c = 0x20; c < 0x180; c++) if (c !== 0x7f) latin.push(String.fromCharCode(c));
  const ascii = latin.filter((c) => c.charCodeAt(0) < 0x7f);
  const marks = ['̀', '́', '̂', '̃', '̈', '̊', '̧', '̸'];
  const controls = ['\u0001', '\t', '\n', '­'];
  const make = (pool, len) => Array.from({ length: 1 + r(len) }, () => pool[r(pool.length)]).join('');
  const groups = {
    ascii: () => make(ascii, 8),
    'ascii-paths': () => make(['a', 'A', 'b', 'B', '/', '-', '_', '.', ' ', '0', '1'], 12),
    latin: () => make(latin, 6),
    'latin-close': () => make(['a', 'A', 'à', 'À', 'á', 'â', 'ä', 'Ä', 'å', 'ã', 'æ', 'Æ', 'e', 'é', 'È', 'o', 'ø', 'Ø', 'ö', 's', 'ß', 'S', 'ſ', 'd', 'đ', 'ð', 'Đ', 'l', 'ł', 'Ł', 'ŀ', 'i', 'ı', 'İ', 'n', 'ñ', 'ŋ', 'ŉ', 'ª', 'º', '²', '¼', 'µ', 'œ', 'Œ', 'þ', 'Þ', 'ĳ', 'Ĳ'], 4),
    marks: () => make(['a', 'e', 'A', ...marks], 5),
    controls: () => make(['a', 'b', ...controls], 5),
  };
  const counts = {};
  const file = path.join(GEN, 'sort-cases.ndjson');
  const lines = [];
  const expected = [];
  const labels = [];
  for (const [label, gen] of Object.entries(groups)) {
    for (let i = 0; i < 400; i++) {
      const list = Array.from({ length: 30 }, gen);
      lines.push(JSON.stringify({ list }));
      expected.push(JSON.stringify([...list].sort((a, b) => a.localeCompare(b, 'en'))));
      labels.push(label);
    }
  }
  fs.writeFileSync(file, lines.join('\n') + '\n');
  const got = execFileSync(BIN, ['sort', '--cases', file], { encoding: 'utf8', maxBuffer: 1 << 28 }).split('\n');
  let bad = 0;
  expected.forEach((e, i) => {
    counts[labels[i]] ??= [0, 0];
    counts[labels[i]][0]++;
    if (e !== got[i]) {
      counts[labels[i]][1]++;
      bad++;
      if (bad <= 20) {
        const a = JSON.parse(e);
        const b = JSON.parse(got[i]);
        const k = a.findIndex((x, j) => x !== b[j]);
        note(`--- sort ${labels[i]} at ${k}: oracle=${JSON.stringify(a.slice(k, k + 3))} lq=${JSON.stringify(b.slice(k, k + 3))}`);
      }
    }
  });
  log(`[sort] cases=${expected.length} diffs=${bad} (${Object.entries(counts).map(([k, v]) => `${k} ${v[1]}/${v[0]}`).join(', ')})`);
}

writeRegistry();
await runDiscovery(console.log);
runMinimatch(console.log);
runIgnored(console.log);
runSort(console.log);
fs.closeSync(diffs);
console.log(`  details: ${path.relative(ROOT, DIFFS)}`);
