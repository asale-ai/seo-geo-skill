#!/usr/bin/env node
// Fetch the prebuilt seogeo binary that matches this package's version and
// this machine's platform.
//
// The npm package carries no binary of its own. Publishing seven platform
// builds as npm artifacts would mean seven packages to keep in step with the
// GitHub release; downloading the release asset keeps one source of truth and
// lets the checksum published beside it do the verifying.
//
// No dependencies. Everything here is Node's standard library plus whichever
// of curl/wget/fetch the machine already has.

'use strict';

const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const { execFileSync, spawnSync } = require('node:child_process');

const REPO = 'asale-ai/seo-geo-skill';
const BIN_NAME = 'seogeo';
const pkg = require('../package.json');

const ROOT = path.join(__dirname, '..');
const VENDOR = path.join(ROOT, 'vendor');
const EXE = process.platform === 'win32' ? `${BIN_NAME}.exe` : BIN_NAME;
const BINARY = path.join(VENDOR, EXE);

/** Rust target triple for this machine, matching the release asset names. */
function detectTarget() {
  const arch = { x64: 'x86_64', arm64: 'aarch64' }[process.arch];
  if (!arch) {
    throw new Error(
      `unsupported architecture: ${process.arch}. seogeo ships x86_64 and aarch64 builds.\n` +
        `Build from source instead: cargo install ${BIN_NAME}`
    );
  }
  if (process.platform === 'darwin') return `${arch}-apple-darwin`;
  if (process.platform === 'win32') {
    if (arch !== 'x86_64') {
      throw new Error(
        'the release does not include an aarch64 Windows build.\n' +
          `Build from source instead: cargo install ${BIN_NAME}`
      );
    }
    return 'x86_64-pc-windows-msvc';
  }
  if (process.platform === 'linux') {
    // A musl build runs on glibc systems too; a gnu build does not run on
    // Alpine. Detect rather than guess, and fall back to musl, which is the
    // safer of the two. `glibcVersionRuntime` is absent on musl.
    const report = typeof process.report?.getReport === 'function' ? process.report.getReport() : null;
    const glibc = report && report.header && report.header.glibcVersionRuntime;
    return `${arch}-unknown-linux-${glibc ? 'gnu' : 'musl'}`;
  }
  throw new Error(`unsupported platform: ${process.platform}`);
}

function have(cmd) {
  const probe = process.platform === 'win32' ? 'where' : 'which';
  return spawnSync(probe, [cmd], { stdio: 'ignore' }).status === 0;
}

/**
 * Download to a file.
 *
 * curl and wget are tried before `fetch` because they honour HTTPS_PROXY,
 * and a proxied network is exactly where a silent download failure is most
 * likely — Node's fetch ignores the proxy environment entirely.
 */
async function download(url, dest) {
  if (have('curl')) {
    execFileSync('curl', ['-fsSL', '--retry', '3', '--retry-delay', '2', '-o', dest, url], {
      stdio: ['ignore', 'ignore', 'pipe'],
    });
    return;
  }
  if (have('wget')) {
    execFileSync('wget', ['-qO', dest, url], { stdio: ['ignore', 'ignore', 'pipe'] });
    return;
  }
  const res = await fetch(url, { redirect: 'follow' });
  if (!res.ok) throw new Error(`HTTP ${res.status} for ${url}`);
  fs.writeFileSync(dest, Buffer.from(await res.arrayBuffer()));
}

function sha256(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

function extract(archive, into) {
  if (archive.endsWith('.zip')) {
    // PowerShell ships with every supported Windows; `tar` there cannot read
    // this zip reliably across versions.
    execFileSync(
      'powershell',
      ['-NoProfile', '-NonInteractive', '-Command', `Expand-Archive -LiteralPath '${archive}' -DestinationPath '${into}' -Force`],
      { stdio: ['ignore', 'ignore', 'pipe'] }
    );
  } else {
    execFileSync('tar', ['xzf', archive, '-C', into], { stdio: ['ignore', 'ignore', 'pipe'] });
  }
}

/** True when a usable binary of the right version is already in place. */
function alreadyInstalled() {
  if (!fs.existsSync(BINARY)) return false;
  const out = spawnSync(BINARY, ['--version'], { encoding: 'utf8' });
  return out.status === 0 && String(out.stdout).includes(pkg.version);
}

async function install({ quiet = false } = {}) {
  if (alreadyInstalled()) return BINARY;

  const target = detectTarget();
  const version = pkg.version;
  const stem = `${BIN_NAME}-${version}-${target}`;
  const asset = target.includes('windows') ? `${stem}.zip` : `${stem}.tar.gz`;
  const base = `https://github.com/${REPO}/releases/download/v${version}`;
  const log = quiet ? () => {} : (m) => process.stderr.write(`${m}\n`);

  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'seogeo-'));
  try {
    log(`seogeo ${version} — downloading ${target}`);
    const archive = path.join(tmp, asset);
    await download(`${base}/${asset}`, archive);

    // Verify against the checksum file published beside the asset. A failed
    // verification aborts; a missing SHA256SUMS only warns, because the
    // alternative is refusing to install from an older release that predates
    // it, and that trade was already made in install.sh.
    try {
      const sums = path.join(tmp, 'SHA256SUMS');
      await download(`${base}/SHA256SUMS`, sums);
      const line = fs
        .readFileSync(sums, 'utf8')
        .split('\n')
        .find((l) => l.trim().endsWith(` ${asset}`) || l.trim().endsWith(`*${asset}`));
      if (line) {
        const expected = line.trim().split(/\s+/)[0];
        const actual = sha256(archive);
        if (expected !== actual) {
          throw new Error(
            `checksum mismatch for ${asset}\n  expected ${expected}\n  actual   ${actual}\n` +
              'Nothing was installed. This could mean a corrupted download or a tampered release.'
          );
        }
        log('  checksum verified');
      } else {
        log(`  warning: ${asset} is not listed in SHA256SUMS; continuing unverified`);
      }
    } catch (e) {
      if (String(e.message).startsWith('checksum mismatch')) throw e;
      log('  warning: could not verify the checksum; continuing');
    }

    extract(archive, tmp);
    const src = path.join(tmp, stem, EXE);
    if (!fs.existsSync(src)) throw new Error(`${asset} did not contain ${EXE}`);

    fs.mkdirSync(VENDOR, { recursive: true });
    fs.copyFileSync(src, BINARY);
    if (process.platform !== 'win32') fs.chmodSync(BINARY, 0o755);

    const check = spawnSync(BINARY, ['--version'], { encoding: 'utf8' });
    if (check.status !== 0) {
      throw new Error(`the downloaded binary will not run here — detected ${target}`);
    }
    log(`  installed ${String(check.stdout).trim()}`);
    return BINARY;
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
}

module.exports = { install, BINARY, detectTarget };

if (require.main === module) {
  install().catch((e) => {
    // A failed postinstall must not fail the whole `npm install`. The launcher
    // retries on first run, so an offline install still works once the machine
    // is back online.
    process.stderr.write(
      `\nseogeo: could not download the binary — ${e.message}\n` +
        'It will be retried the first time you run seogeo.\n\n'
    );
    process.exit(0);
  });
}
