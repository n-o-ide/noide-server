#!/usr/bin/env node

const fs = require('fs');
const path = require('path');
const os = require('os');
const https = require('https');
const http = require('http');
const crypto = require('crypto');
const { spawn, execSync } = require('child_process');

const pkg = require('../package.json');
const VERSION = pkg.version;
const REPO = 'nio-labs/nio-de';
// Releases before 0.5 used the former artifact name.
const [major, minor] = VERSION.split('.').map(Number);
const nativeName = major === 0 && minor < 5 ? 'noide-server' : 'nio-de';

function getPlatformInfo() {
  const platform = os.platform();
  const arch = os.arch();

  let osSuffix = '';
  if (platform === 'linux') osSuffix = 'linux';
  else if (platform === 'darwin') osSuffix = 'darwin';
  else if (platform === 'win32') osSuffix = 'windows';
  else {
    console.error(`[nio-de] Error: Unsupported OS: ${platform}`);
    process.exit(1);
  }

  let archSuffix = '';
  if (arch === 'x64') archSuffix = 'x86_64';
  else if (arch === 'arm64') archSuffix = 'aarch64';
  else {
    console.error(`[nio-de] Error: Unsupported architecture: ${arch}`);
    process.exit(1);
  }

  const ext = platform === 'win32' ? '.exe' : '';
  const assetName = `${nativeName}-${osSuffix}-${archSuffix}${ext}`;
  const binName = `nio-de${ext}`;

  return { osSuffix, archSuffix, ext, assetName, binName };
}

function fetchWithRedirects(url, maxRedirects = 5) {
  return new Promise((resolve, reject) => {
    if (maxRedirects <= 0) {
      return reject(new Error('Too many redirects while downloading binary'));
    }

    const client = url.startsWith('https:') ? https : http;
    const req = client.get(url, { headers: { 'User-Agent': `nio-de-npm/${VERSION}` } }, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        return resolve(fetchWithRedirects(res.headers.location, maxRedirects - 1));
      }
      if (res.statusCode !== 200) {
        return reject(new Error(`Failed to download: HTTP ${res.statusCode} from ${url}`));
      }
      resolve(res);
    });

    req.on('error', reject);
  });
}

function streamToString(stream) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    stream.on('data', (d) => chunks.push(d));
    stream.on('end', () => resolve(Buffer.concat(chunks).toString('utf-8')));
    stream.on('error', reject);
  });
}

async function ensureBinary() {
  const overrideBin = process.env.NIO_DE_SERVER_BIN || process.env.NOIDE_SERVER_BIN;
  // 1. Check custom override env var
  if (overrideBin) {
    if (fs.existsSync(overrideBin)) {
      return overrideBin;
    }
    console.warn(`[nio-de] Warning: NIO_DE_SERVER_BIN / NOIDE_SERVER_BIN was set to "${overrideBin}" but file does not exist.`);
  }

  const { assetName, binName, ext } = getPlatformInfo();

  // 2. Check system PATH if nio-de is already installed and matches version
  try {
    const sysBin = execSync(os.platform() === 'win32' ? 'where nio-de' : 'which nio-de', { stdio: ['pipe', 'pipe', 'ignore'] }).toString().trim().split(/\r?\n/)[0];
    if (sysBin && fs.existsSync(sysBin)) {
      try {
        const verOutput = execSync(`"${sysBin}" --version`, { stdio: ['pipe', 'pipe', 'ignore'] }).toString();
        if (verOutput.includes(VERSION)) {
          return sysBin;
        }
      } catch {
        // Fall back to downloaded binary
      }
    }
  } catch {
    // Not found in system PATH
  }

  // 3. Cache directory: ~/.noide/bin/nio-de-vX.Y.Z
  const cacheDir = path.join(os.homedir(), '.noide', 'bin');
  fs.mkdirSync(cacheDir, { recursive: true });

  const targetBinPath = path.join(cacheDir, `nio-de-v${VERSION}${ext}`);
  if (fs.existsSync(targetBinPath)) {
    try {
      fs.accessSync(targetBinPath, fs.constants.X_OK);
      return targetBinPath;
    } catch {
      // not executable, re-chmod
      fs.chmodSync(targetBinPath, 0o755);
      return targetBinPath;
    }
  }

  // 4. Download binary from GitHub Release
  const tag = `v${VERSION}`;
  const baseUrl = `https://github.com/${REPO}/releases/download/${tag}`;
  const binUrl = `${baseUrl}/${assetName}`;
  const sumsUrl = `${baseUrl}/SHA256SUMS`;

  console.log(`[nio-de] Downloading native binary (${tag}, ${assetName})...`);

  // Fetch SHA256SUMS
  let expectedHash = null;
  try {
    const sumsRes = await fetchWithRedirects(sumsUrl);
    const sumsText = await streamToString(sumsRes);
    for (const line of sumsText.split('\n')) {
      const parts = line.trim().split(/\s+/);
      if (parts.length >= 2) {
        const hash = parts[0];
        const file = parts[1].replace(/^\*/, '');
        if (file === assetName) {
          expectedHash = hash;
          break;
        }
      }
    }
  } catch (err) {
    console.warn(`[nio-de] Warning: Could not fetch SHA256SUMS: ${err.message}`);
  }

  // Fetch binary
  const tempPath = path.join(cacheDir, `.download-${Date.now()}-${binName}`);
  const binRes = await fetchWithRedirects(binUrl);

  const fileStream = fs.createWriteStream(tempPath);
  const hasher = crypto.createHash('sha256');

  await new Promise((resolve, reject) => {
    binRes.on('data', (chunk) => {
      hasher.update(chunk);
      fileStream.write(chunk);
    });
    binRes.on('end', () => {
      fileStream.end(resolve);
    });
    binRes.on('error', (err) => {
      fileStream.destroy();
      fs.unlink(tempPath, () => {});
      reject(err);
    });
  });

  const actualHash = hasher.digest('hex');
  if (expectedHash && actualHash !== expectedHash) {
    fs.unlinkSync(tempPath);
    throw new Error(`Checksum verification failed for ${assetName}! Expected ${expectedHash}, got ${actualHash}`);
  }

  fs.chmodSync(tempPath, 0o755);
  fs.renameSync(tempPath, targetBinPath);
  console.log(`[nio-de] Native binary ready.`);

  return targetBinPath;
}

async function ensureNioBinary() {
  // 1. Check system PATH
  try {
    const cmd = os.platform() === 'win32' ? 'where nio' : 'which nio';
    const nioBin = execSync(cmd, { stdio: ['pipe', 'pipe', 'ignore'] }).toString().trim().split(/\r?\n/)[0];
    if (nioBin && fs.existsSync(nioBin)) return nioBin;
  } catch {}

  // 2. Check canonical install locations
  const isWin = os.platform() === 'win32';
  const ext = isWin ? '.exe' : '';
  const candidates = [
    path.join(os.homedir(), '.nio', 'bin', `nio${ext}`),
    path.join(os.homedir(), '.local', 'bin', `nio${ext}`),
    path.join(os.homedir(), '.cargo', 'bin', `nio${ext}`)
  ];
  for (const c of candidates) {
    if (fs.existsSync(c)) return c;
  }

  // 3. Not found — install nio-ai (npm first, then curl fallback)
  console.log('[nio-de] Bundling required NioAI agent (nio-ai)...');

  // Attempt A: via npx / npm
  try {
    execSync('npx -y nio-ai --version', { stdio: ['pipe', 'pipe', 'ignore'], timeout: 30000 });
    for (const c of candidates) {
      if (fs.existsSync(c)) return c;
    }
  } catch {}

  // Attempt B: direct native curl/powershell fallback
  try {
    if (isWin) {
      execSync('powershell -NoProfile -Command "irm https://raw.githubusercontent.com/nio-labs/nio/main/install.ps1 | iex"', {
        stdio: ['pipe', 'inherit', 'inherit'],
        timeout: 60000
      });
    } else {
      execSync('curl -fsSL https://raw.githubusercontent.com/nio-labs/nio/main/install.sh | bash', {
        stdio: ['pipe', 'inherit', 'inherit'],
        timeout: 60000
      });
    }
    for (const c of candidates) {
      if (fs.existsSync(c)) return c;
    }
  } catch (err) {
    console.warn(`[nio-de] Warning: Could not automatically bundle nio-ai: ${err.message}`);
    console.warn('[nio-de] You can install it manually with: npx nio-ai');
  }

  return null;
}

async function main() {
  try {
    const binPath = await ensureBinary();
    // Ensure NioAI agent is bundled and ready
    await ensureNioBinary();

    const args = process.argv.slice(2);

    const child = spawn(binPath, args, {
      stdio: 'inherit',
      cwd: process.cwd(),
      env: process.env
    });

    const forwardSignal = (sig) => {
      if (child.pid) {
        try {
          process.kill(child.pid, sig);
        } catch {}
      }
    };

    process.on('SIGINT', () => forwardSignal('SIGINT'));
    process.on('SIGTERM', () => forwardSignal('SIGTERM'));
    process.on('SIGHUP', () => forwardSignal('SIGHUP'));

    child.on('exit', (code) => {
      process.exit(code ?? 0);
    });

    child.on('error', (err) => {
      console.error(`[nio-de] Failed to run binary: ${err.message}`);
      process.exit(1);
    });
  } catch (err) {
    console.error(`[nio-de] Initialization error: ${err.message}`);
    process.exit(1);
  }
}

main();

