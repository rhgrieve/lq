#!/usr/bin/env node
// Builds the npm tarball that releases attach, from binaries already built.
//   scripts/pack-npm.mjs --vendor DIR --out DIR --upstream DIR [--version X.Y.Z] [--allow-missing]
// DIR/<platform>-<arch>/lq holds each binary, with names as in Node's process.platform and
// process.arch. Every platform is required unless --allow-missing, which is for local tests.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const PLATFORMS = ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64'];
const NAME = 'lq-theme-check';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const val = (f) => (args.includes(f) ? args[args.indexOf(f) + 1] : undefined);
const fail = (m) => {
  console.error(m);
  process.exit(1);
};

const cargoToml = fs.readFileSync(path.join(ROOT, 'Cargo.toml'), 'utf8');
const cargoVersion = cargoToml.match(/\[workspace\.package\][^[]*?version = "([^"]+)"/)?.[1];
const version = val('--version') ?? cargoVersion;
if (version !== cargoVersion) fail(`Version ${version} doesn't match Cargo.toml's ${cargoVersion}`);
const vendor = val('--vendor');
const out = val('--out');
const upstream = val('--upstream');
if (!vendor || !out || !upstream) fail('usage: pack-npm.mjs --vendor DIR --out DIR --upstream DIR [--version X.Y.Z] [--allow-missing]');

const found = PLATFORMS.filter((p) => fs.existsSync(path.join(vendor, p, 'lq')));
const absent = PLATFORMS.filter((p) => !found.includes(p));
if (absent.length && !args.includes('--allow-missing')) fail(`Missing binaries: ${absent.join(', ')}`);
if (!found.length) fail('No binaries found');

const work = fs.mkdtempSync(path.join(os.tmpdir(), 'lq-pack-'));
fs.cpSync(path.join(ROOT, 'npm'), work, { recursive: true });
for (const f of ['LICENSE', 'NOTICE.md', 'DIVERGENCES.md']) fs.copyFileSync(path.join(ROOT, f), path.join(work, f));
const licenses = execFileSync(process.execPath, [path.join(ROOT, 'scripts/third-party-licenses.mjs'), path.resolve(upstream)], { encoding: 'utf8', maxBuffer: 1 << 28 });
fs.writeFileSync(path.join(work, 'THIRD_PARTY_LICENSES.md'), licenses);
for (const p of found) {
  fs.mkdirSync(path.join(work, 'vendor', p), { recursive: true });
  fs.copyFileSync(path.join(vendor, p, 'lq'), path.join(work, 'vendor', p, 'lq'));
  fs.chmodSync(path.join(work, 'vendor', p, 'lq'), 0o755);
}
fs.chmodSync(path.join(work, 'bin/lq.js'), 0o755);
fs.writeFileSync(
  path.join(work, 'package.json'),
  `${JSON.stringify(
    {
      name: NAME,
      version,
      // Installed from release URLs; `private` stops an accidental `npm publish`.
      private: true,
      description: 'A fast native clone of shopify theme check and the Liquid Prettier plugin',
      license: 'MIT',
      bin: { lq: 'bin/lq.js' },
      os: ['darwin', 'linux'],
      cpu: ['arm64', 'x64'],
      files: ['bin', 'vendor', 'LICENSE', 'NOTICE.md', 'DIVERGENCES.md', 'THIRD_PARTY_LICENSES.md'],
    },
    null,
    2,
  )}\n`,
);
fs.mkdirSync(out, { recursive: true });
const tarball = execFileSync('npm', ['pack', '--silent', '--pack-destination', path.resolve(out)], { cwd: work, encoding: 'utf8' }).trim().split('\n').pop();
fs.rmSync(work, { recursive: true, force: true });
console.log(path.join(path.resolve(out), tarball));
