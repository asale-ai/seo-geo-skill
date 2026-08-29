#!/usr/bin/env node
// Launcher: hand every argument to the real binary and exit with its status.
//
// The download normally happens in postinstall. It is retried here because
// `npm install --ignore-scripts` and some CI sandboxes skip lifecycle scripts
// entirely, and a tool that fails with "command not found" in that case is a
// tool people stop using.

'use strict';

const { spawn } = require('node:child_process');
const fs = require('node:fs');
const { install, BINARY } = require('../lib/install.js');

function run(binary) {
  const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit' });
  child.on('error', (e) => {
    process.stderr.write(`seogeo: ${e.message}\n`);
    process.exit(1);
  });
  // Re-raise the child's signal so Ctrl-C behaves the way it would if the
  // binary had been invoked directly.
  child.on('exit', (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code === null ? 1 : code);
  });
}

if (fs.existsSync(BINARY)) {
  run(BINARY);
} else {
  install()
    .then(run)
    .catch((e) => {
      process.stderr.write(`seogeo: ${e.message}\n`);
      process.exit(1);
    });
}
