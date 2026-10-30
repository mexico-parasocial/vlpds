// Isolated VLPDS -> patched rsky -> WatZappa decoder/lexicon integration.
// Uses ephemeral ports, test keys and a temporary relay directory. No existing
// deployment or database is touched. Node >=22; install WatZappa dependencies.
import assert from 'node:assert/strict'
import { spawn, execFileSync } from 'node:child_process'
import { once } from 'node:events'
import { createServer } from 'node:http'
import { mkdtempSync, mkdirSync, createWriteStream, chmodSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve, dirname } from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath, pathToFileURL } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const root = resolve(here, '../../..')
const watzappa = resolve(process.env.WATZAPPA_DIR || resolve(root, '../WatZappa'))
const require = createRequire(resolve(watzappa, 'packages/xrpc-server/package.json'))
const WebSocket = require('ws')
const { decodeAll, encode } = require('@atproto/lex-cbor')
const { com } = await import(pathToFileURL(resolve(watzappa, 'packages/sync/dist/lexicons/index.js')))
const state = mkdtempSync(resolve(tmpdir(), 'para-rsky-smoke-'))
const relayState = resolve(state, 'relay')
mkdirSync(relayState)
chmodSync(relayState, 0o777) // Docker's test-only uid 10001 writes this directory.
const image = process.env.RELAY_IMAGE
const children = []
const sockets = []
const dockerNames = []
let plcAvailable = true
let decodingFailure
const operations = new Map()
const sleep = (ms) => new Promise((done) => setTimeout(done, ms))

function run(binary, args, env, log, cwd = state) {
  const child = spawn(binary, args, { cwd, env: { ...process.env, ...env }, stdio: ['ignore', 'pipe', 'pipe'] })
  const stream = createWriteStream(resolve(state, log))
  child.stdout.pipe(stream)
  child.stderr.pipe(stream)
  children.push(child)
  return child
}

async function waitFor(check, label, timeout = 20_000) {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    if (decodingFailure) throw decodingFailure
    if (await check()) return
    await sleep(100)
  }
  throw new Error(`Timed out: ${label}; logs: ${state}`)
}

async function freePort() {
  const server = createServer()
  server.listen(0, '127.0.0.1')
  await once(server, 'listening')
  const port = server.address().port
  await new Promise((done) => server.close(done))
  return port
}

async function request(base, method, body, jwt) {
  const response = await fetch(`${base}/xrpc/${method}`, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { 'content-type': 'application/json', ...(jwt ? { authorization: `Bearer ${jwt}` } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  assert.equal(response.status, 200, `${method}: ${await response.clone().text()}`)
  const text = await response.text()
  return text ? JSON.parse(text) : undefined
}

async function subscribe(url, validate = true) {
  const events = []
  const ws = new WebSocket(url)
  ws.on('error', (error) => events.push({ connectionError: error.message }))
  sockets.push(ws)
  ws.on('message', (bytes) => {
    try {
    const [header, body] = decodeAll(new Uint8Array(bytes))
    if (header.op === -1) { events.push({ error: body.error }); return }
    if (validate) {
      const parsed = com.atproto.sync.subscribeRepos.$message.safeParse({ ...body, $type: `com.atproto.sync.subscribeRepos${header.t}` })
      assert.equal(parsed.success, true, `WatZappa rejected ${header.t}: ${JSON.stringify(parsed.error)}`)
      if ('seq' in body) assert(Number.isSafeInteger(body.seq), 'downstream cursor must be a safe JS integer')
    }
    events.push({ type: header.t, body })
    } catch (error) { decodingFailure = error }
  })
  await once(ws, 'open')
  return { ws, events }
}

const pdsPort = await freePort()
const relayPort = await freePort()
const pds = `http://localhost:${pdsPort}`
const relay = `http://localhost:${relayPort}`
const plc = createServer(async (req, res) => {
  // createBoard checks the AppView's global name registry. This fixture
  // isolates the relay; it does not claim to exercise AppView indexing.
  if (req.url.startsWith('/xrpc/com.para.community.listBoards')) {
    res.writeHead(200, { 'content-type': 'application/json' }).end('{"boards":[]}')
    return
  }
  const did = decodeURIComponent(req.url.slice(1))
  if (req.method === 'POST') {
    let body = ''
    for await (const chunk of req) { body += chunk; assert(body.length < 1_000_000) }
    operations.set(did, JSON.parse(body))
    res.writeHead(200, { 'content-type': 'application/json' }).end('{}')
    return
  }
  const op = operations.get(did)
  if (!plcAvailable || !op) { res.writeHead(503).end(); return }
  res.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({
    id: did,
    alsoKnownAs: op.alsoKnownAs,
    verificationMethod: [{ id: `${did}#atproto`, controller: did, type: 'Multikey', publicKeyMultibase: op.verificationMethods.atproto.replace('did:key:', '') }],
    service: [{ id: `${did}#atproto_pds`, type: 'AtprotoPersonalDataServer', serviceEndpoint: op.services.atproto_pds.endpoint }],
  }))
})
plc.listen(0, '0.0.0.0')
await once(plc, 'listening')
const plcPort = plc.address().port
// Transparent wire proxy lets the test inject the upstream history-gap signal.
const proxy = new WebSocket.Server({ host: '0.0.0.0', port: 0 })
proxy.on('connection', (front, request) => {
  const back = new WebSocket(`ws://localhost:${pdsPort}${request.url}`)
  back.on('message', (bytes) => { if (front.readyState === WebSocket.OPEN) front.send(bytes) })
  back.on('error', () => front.close())
  back.on('close', () => front.close())
  front.on('close', () => back.terminate())
  front.on('error', () => back.terminate())
})
await once(proxy, 'listening')
const proxyPort = proxy.address().port
let relayProcess
let currentDockerName
let generation = 0

async function startRelay() {
  generation += 1
  const env = {
    RELAY_LISTEN: `0.0.0.0:${image ? 2470 : relayPort}`,
    RELAY_PDS_HOST: `localhost:${pdsPort}`,
    RELAY_UPSTREAM_URL: `http://${image ? 'host.docker.internal' : 'localhost'}:${proxyPort}`,
    RELAY_PLC_URL: `http://${image ? 'host.docker.internal' : 'localhost'}:${plcPort}`,
    RELAY_LENIENT_VALIDATION: 'false',
    RELAY_ADMIN_PASSWORD: 'isolated-relay-test-password',
    RUST_LOG: 'rsky_relay=debug',
  }
  if (image) {
    currentDockerName = `para-rsky-smoke-${process.pid}-${generation}`
    dockerNames.push(currentDockerName)
    relayProcess = run('docker', ['run', '--name', currentDockerName, '-p', `127.0.0.1:${relayPort}:2470`, '-v', `${relayState}:/data/relay`, ...Object.entries(env).flatMap(([key, value]) => ['-e', `${key}=${value}`]), image], {}, `relay-${generation}.log`)
  } else {
    relayProcess = run(resolve(process.env.RELAY_BIN || resolve(root, 'target/rsky-audit/target/debug/rsky-relay')), ['--no-plc-export'], env, `relay-${generation}.log`, relayState)
  }
  await waitFor(async () => {
    if (relayProcess.exitCode !== null) throw new Error(`relay exited ${relayProcess.exitCode}; logs: ${state}`)
    return fetch(`${relay}/xrpc/_health`).then((r) => r.ok).catch(() => false)
  }, 'relay health')
}

async function stopRelay(hard) {
  const ended = once(relayProcess, 'exit')
  if (image) execFileSync('docker', hard ? ['kill', currentDockerName] : ['stop', '-t', '15', currentDockerName], { stdio: 'ignore' })
  else relayProcess.kill(hard ? 'SIGKILL' : 'SIGTERM')
  await ended
}

try {
  const pdsProcess = run(resolve(process.env.VLPDS_BIN || resolve(root, 'target/debug/vlpds')), [
    '--memory', '--dev-mode', '--no-rate-limits', '--crawlers=', '--para',
    `--listen=0.0.0.0:${pdsPort}`, `--public-url=${pds}`, '--handle-domain=test',
    `--plc-url=http://localhost:${plcPort}`, '--plc-mode=directory',
    `--appview=http://localhost:${plcPort},did:web:appview.test`,
  ], { VLPDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX: '11'.repeat(32) }, 'vlpds.log')
  await waitFor(async () => {
    if (pdsProcess.exitCode !== null) throw new Error(`VLPDS exited ${pdsProcess.exitCode}; logs: ${state}`)
    return fetch(`${pds}/xrpc/_health`).then((r) => r.ok).catch(() => false)
  }, 'VLPDS health')
  const raw = await subscribe(`ws://localhost:${pdsPort}/xrpc/com.atproto.sync.subscribeRepos?cursor=0`, false)
  await startRelay()
  const live = await subscribe(`ws://localhost:${relayPort}/xrpc/com.atproto.sync.subscribeRepos?cursor=0`)
  const session = await request(pds, 'com.atproto.server.createAccount', { handle: 'relay-smoke.test', email: 'relay@example.invalid', password: 'isolated-native-test-password' })
  const board = await request(pds, 'com.para.community.createBoard', { name: 'Native relay smoke', quadrant: 'MX-NL' }, session.accessJwt)
  const record = { $type: 'app.bsky.feed.post', text: 'create through rsky', createdAt: new Date().toISOString() }
  const post = await request(pds, 'com.atproto.repo.createRecord', { repo: session.did, collection: 'app.bsky.feed.post', record }, session.accessJwt)
  const key = post.uri.split('/').at(-1)
  await request(pds, 'com.atproto.repo.putRecord', { repo: session.did, collection: 'app.bsky.feed.post', rkey: key, record: { ...record, text: 'updated through rsky' } }, session.accessJwt)
  await request(pds, 'com.atproto.repo.deleteRecord', { repo: session.did, collection: 'app.bsky.feed.post', rkey: key }, session.accessJwt)
  const isDelete = (event) => event.type === '#commit' && event.body.ops.some((op) => op.action === 'delete')
  await waitFor(() => live.events.some(isDelete), 'PARA and post create/update/delete delivery')
  const commits = live.events.filter((event) => event.type === '#commit')
  assert(commits.some(({ body }) => body.ops.length === 8 && body.ops.some((op) => `at://${session.did}/${op.path}` === board.uri)), 'native community must remain one eight-record commit')
  for (const { body } of commits) {
    const original = raw.events.find((event) => event.type === '#commit' && event.body.commit.toString() === body.commit.toString())?.body
    assert(original, 'relay commit must exist in the upstream stream')
    assert.equal(typeof original.seq, 'bigint')
    assert(original.seq > BigInt(Number.MAX_SAFE_INTEGER))
    assert.deepEqual(body.blocks, original.blocks, 'CAR bytes and signed commit must stay unchanged')
    assert.equal(body.ops.length, original.ops.length)
    for (let i = 0; i < body.ops.length; i++) {
      const op = body.ops[i], before = original.ops[i]
      assert.equal(op.action, before.action)
      assert.equal(op.path, before.path)
      if (op.action !== 'create') assert.equal(op.prev.toString(), before.prev.toString(), 'previous CID must survive forwarding')
      if (op.action === 'delete') assert.equal(op.cid, null)
      else assert.equal(op.cid.toString(), before.cid.toString())
    }
  }
  console.log('PASS: native eight-record PARA commit, create/update/delete, exact CAR/previous CIDs and WatZappa schemas')
  const saved = new Map(commits.map(({ body }) => [body.commit.toString(), body.seq]))
  const tail = Math.max(...live.events.filter((event) => event.body?.seq).map((event) => event.body.seq))
  const forbidden = await fetch(`${relay}/xrpc/com.atproto.sync.requestCrawl`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ hostname: 'attacker.example' }) })
  assert.equal(forbidden.status, 403)
  const future = await subscribe(`ws://localhost:${relayPort}/xrpc/com.atproto.sync.subscribeRepos?cursor=${tail + 1000}`)
  await waitFor(() => future.events.some((event) => event.error === 'FutureCursor'), 'future cursor rejection')
  await stopRelay(true)
  await startRelay()
  const replay = await subscribe(`ws://localhost:${relayPort}/xrpc/com.atproto.sync.subscribeRepos?cursor=0`)
  await waitFor(() => replay.events.some(isDelete), 'replay after SIGKILL')
  for (const { body } of replay.events.filter((event) => event.type === '#commit')) {
    if (saved.has(body.commit.toString())) assert.equal(body.seq, saved.get(body.commit.toString()), 'delivered cursors must survive SIGKILL unchanged')
  }
  const after = await request(pds, 'com.atproto.repo.createRecord', { repo: session.did, collection: 'app.bsky.feed.post', record: { ...record, text: 'after crash' } }, session.accessJwt)
  await waitFor(() => replay.events.some((event) => event.type === '#commit' && event.body.commit.toString() !== undefined && event.body.ops.some((op) => `at://${session.did}/${op.path}` === after.uri)), 'new write after restart')
  assert(replay.events.some((event) => event.type === '#commit' && event.body.seq > tail), 'restart must not reuse prior output cursors')
  console.log('PASS: allowlist, FutureCursor, durable SIGKILL replay and continued ingestion')
  plcAvailable = false
  const queued = await request(pds, 'com.atproto.server.createAccount', { handle: 'queued-smoke.test', email: 'queued@example.invalid', password: 'isolated-native-test-password' })
  const pending = await request(pds, 'com.atproto.repo.createRecord', { repo: queued.did, collection: 'app.bsky.feed.post', record: { ...record, text: 'queued while PLC unavailable' } }, queued.accessJwt)
  await sleep(2000)
  assert(!replay.events.some((event) => event.type === '#commit' && event.body.repo === queued.did), 'unresolved signing identity must not be published')
  await stopRelay(true)
  await startRelay()
  const recovered = await subscribe(`ws://localhost:${relayPort}/xrpc/com.atproto.sync.subscribeRepos?cursor=0`)
  plcAvailable = true
  await waitFor(() => recovered.events.some((event) => event.type === '#commit' && event.body.repo === queued.did && event.body.ops.some((op) => `at://${queued.did}/${op.path}` === pending.uri)), 'queued identity recovery after SIGKILL without another PDS write', 25_000)
  console.log('PASS: strict identity verification and durable queue recovery after PLC outage and SIGKILL')
  await stopRelay(false)
  console.log('PASS: graceful shutdown')
  await startRelay()
  await waitFor(() => proxy.clients.size > 0, 'upstream subscription after graceful restart')
  const gap = Buffer.concat([encode({ op: 1, t: '#info' }), encode({ name: 'OutdatedCursor' })])
  for (const peer of proxy.clients) peer.send(gap)
  await waitFor(() => relayProcess.exitCode !== null, 'fail closed on an upstream history gap')
  assert.notEqual(relayProcess.exitCode, 0)
  console.log(`PASS: upstream OutdatedCursor stops ingestion explicitly; isolated logs: ${state}`)
} finally {
  for (const ws of sockets) ws.terminate()
  for (const name of dockerNames) { try { execFileSync('docker', ['rm', '-f', name], { stdio: 'ignore' }) } catch {} }
  for (const child of children) if (child.exitCode === null) child.kill('SIGTERM')
  plc.close()
  for (const peer of proxy.clients) peer.terminate()
  proxy.close()
}
