#!/usr/bin/env node
// Smoke test for noide-server. Verifies, against a real binary:
//   1. `--version` prints semver.
//   2. Pairing mode (default): the printed code opens a WS; no/wrong code rejected.
//   3. `--no-auth`: connections accepted without a token.
//   4. `--token <value>`: fixed token accepted, wrong rejected.
//
// No dependencies: performs the WebSocket handshake with a raw `net` socket.
//
// Usage: node scripts/smoke-test.mjs <path-to-noide-server-binary>

import { spawn } from 'node:child_process'
import net from 'node:net'
import crypto from 'node:crypto'

const BIN = process.argv[2]
if (!BIN) {
  console.error('usage: node scripts/smoke-test.mjs <path-to-noide-server-binary>')
  process.exit(2)
}

const CODE_RE = /\b([A-Z2-9]{4}-[A-Z2-9]{4})\b/
const LOG_PREFIX = '[NoIDE]'

let failures = 0
const check = (name, ok, detail = '') => {
  console.log(`${ok ? '  ok' : 'FAIL'}  ${name}${detail ? ` — ${detail}` : ''}`)
  if (!ok) failures++
}

// --- WebSocket handshake probe -------------------------------------------------

// Returns 'open' (101), 'rejected' (HTTP >=400), or 'no-open' (timeout/garbage).
function probe(port, path = '') {
  return new Promise((resolve) => {
    const sock = net.connect({ host: '127.0.0.1', port })
    let buf = ''
    const timer = setTimeout(() => { sock.destroy(); resolve('no-open') }, 3000)
    sock.on('connect', () => {
      sock.write(
        `GET ${path || '/'} HTTP/1.1\r\n` +
          `Host: 127.0.0.1:${port}\r\n` +
          'Upgrade: websocket\r\n' +
          'Connection: Upgrade\r\n' +
          `Sec-WebSocket-Key: ${crypto.randomBytes(16).toString('base64')}\r\n` +
          'Sec-WebSocket-Version: 13\r\n\r\n'
      )
    })
    sock.on('data', (d) => {
      buf += d.toString()
      const head = buf.split('\r\n')[0] || ''
      if (head.includes('101')) { clearTimeout(timer); sock.destroy(); resolve('open') }
      else if (/^HTTP\/1\.1 [4-5]/.test(head)) { clearTimeout(timer); sock.destroy(); resolve('rejected') }
    })
    sock.on('error', () => {})
    sock.on('close', () => { clearTimeout(timer); resolve('no-open') })
  })
}

// --- Server lifecycle -----------------------------------------------------------

function startServer(extraArgs = [], port) {
  const proc = spawn(BIN, [...extraArgs], {
    env: { ...process.env, NOTERM_WS_ADDR: `127.0.0.1:${port}` },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  let stderr = ''
  let resolveListening, resolveExit
  const listening = new Promise((r) => { resolveListening = r })
  const exited = new Promise((r) => { resolveExit = r })
  proc.stderr.on('data', (d) => {
    stderr += d.toString()
    if (stderr.includes('listening on ws://')) resolveListening()
  })
  proc.on('exit', (code) => resolveExit(code))
  return {
    proc,
    port,
    listening,
    exited,
    stderr: () => stderr,
    stop: async () => {
      proc.kill('SIGTERM')
      await Promise.race([exited, new Promise((r) => setTimeout(r, 2000))])
      if (proc.exitCode === null) proc.kill('SIGKILL')
    },
  }
}

const randPort = () => 20000 + Math.floor(Math.random() * 20000)

async function main() {
  // --- 1. --version -------------------------------------------------------------
  console.log('version:')
  const v = spawn(BIN, ['--version'])
  let out = ''
  v.stdout.on('data', (d) => { out += d })
  const versionCode = await new Promise((r) => v.on('exit', r))
  const m = out.trim().match(/^noide-server (\d+\.\d+\.\d+)/)
  check('--version prints semver', versionCode === 0 && !!m, `got "${out.trim()}"`)

  // --- 2. Pairing (default) -------------------------------------------------------
  console.log('pairing mode:')
  const s1 = startServer([], randPort())
  await s1.listening
  const code = CODE_RE.exec(s1.stderr())?.[1]
  check('server printed a pairing code', !!code, code ? `code ${code}` : 'no code found')
  check('no token -> rejected', (await probe(s1.port)) !== 'open')
  check('wrong code -> rejected', (await probe(s1.port, '/?token=AAAA-BBBB')) !== 'open')
  if (code) check('printed code -> open', (await probe(s1.port, `/?token=${code}`)) === 'open', `token=${code}`)
  await s1.stop()

  // --- 3. --no-auth ---------------------------------------------------------------
  console.log('no-auth mode:')
  const s2 = startServer(['--no-auth'], randPort())
  await s2.listening
  check('no token -> open', (await probe(s2.port)) === 'open')
  await s2.stop()

  // --- 4. Fixed --token -------------------------------------------------------------
  console.log('fixed-token mode:')
  const s3 = startServer(['--token', 'fixed-tok-1'], randPort())
  await s3.listening
  check('wrong token -> rejected', (await probe(s3.port, '/?token=wrong')) !== 'open')
  check('correct token -> open', (await probe(s3.port, '/?token=fixed-tok-1')) === 'open')
  await s3.stop()

  console.log(failures === 0 ? '\nSMOKE TEST PASSED' : `\nSMOKE TEST FAILED (${failures})`)
  process.exit(failures === 0 ? 0 : 1)
}

main().catch((e) => { console.error(e); process.exit(1) })
