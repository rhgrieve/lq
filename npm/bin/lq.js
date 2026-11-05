#!/usr/bin/env node
// Runs the native `lq` binary for this platform. Scripts that care about Node's startup time
// can get the binary's path once with `--print-binary-path` and call it directly.
'use strict';
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const platform = `${process.platform}-${process.arch}`;
const binary = path.join(__dirname, '..', 'vendor', platform, 'lq');

if (!fs.existsSync(binary)) {
  const available = fs.readdirSync(path.join(__dirname, '..', 'vendor')).join(', ');
  console.error(`lq: no binary for ${platform}. This package includes: ${available}.`);
  process.exit(2);
}

const args = process.argv.slice(2);
if (args.length === 1 && args[0] === '--print-binary-path') {
  console.log(binary);
  process.exit(0);
}

const result = spawnSync(binary, args, { stdio: 'inherit' });
if (result.error) {
  console.error(`lq: couldn't run ${binary}: ${result.error.message}`);
  process.exit(2);
}
if (result.signal) process.kill(process.pid, result.signal);
process.exit(result.status ?? 1);
