/*
 * make e2e-dm — end-to-end check of Matrix DMs by consent, fully isolated
 * (run through scripts/e2e-dm.sh, which sets BRIDGE_DIR, PARA_DIR and
 * SYNAPSE_IMAGE):
 *   - a throwaway Synapse in Docker (no MAS, password logins),
 *   - the real bridge router (/api/matrix-challenge, /api/dm-requests*) over
 *     HTTP with an in-memory SQLite DB and real sr25519 identity proofs,
 *   - two matrix-js-sdk clients with Rust crypto doing what PARA's native
 *     client does: B creates the encrypted direct room (history "invited"),
 *     invites A and sends the encrypted com.para.dm.intro; A sees the direct
 *     invite, joins, decrypts the intro and matches it with PARA's own
 *     matchDmIntro against the bridge's outgoing list.
 */
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { createHmac, randomUUID } from 'node:crypto'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'

const BRIDGE = process.env.BRIDGE_DIR!
const PARA = process.env.PARA_DIR!
const SYNAPSE_IMAGE = process.env.SYNAPSE_IMAGE!

const { SqliteBridgeDatabase } = await import(`${BRIDGE}/src/db/sqlite-wrapper.ts`)
const { ChallengeStore } = await import(`${BRIDGE}/src/challenge.ts`)
const { routeRequest } = await import(`${BRIDGE}/src/routes/router.ts`)
const { BRIDGE_AUDIENCES, mxidForIdentityPub } = await import(
  `${BRIDGE}/src/identity-proof.ts`
)
const { deriveIdentity, signAssertion } = await import(
  `${BRIDGE}/src/__tests__/helpers/identity.ts`
)
const { dmIntroMessage, matchDmIntro, parseDmIntroBody, DM_INTRO_MSGTYPE } = await import(
  `${PARA}/src/lib/matrix/dmIntro.ts`
)
const sdk = await import(`${PARA}/node_modules/matrix-js-sdk/lib/index.js`)

// One line per step: e2e-dm.sh shows only these lines.
const step = (msg: string, extra?: unknown) =>
  console.log(`[dm-e2e] ${msg}${extra === undefined ? '' : ` ${JSON.stringify(extra)}`}`)

const serverName = 'dm.test'
const sharedSecret = randomUUID()
const container = `para-dm-e2e-${randomUUID().slice(0, 8)}`
const dir = await mkdtemp(join(tmpdir(), 'para-dm-e2e-'))
let started = false
const clients: Array<{ stopClient(): void }> = []
const alice = { did: 'did:plc:alicee2e', token: 'tok-alice' }
const bob = { did: 'did:plc:bobe2e', token: 'tok-bob' }
const carol = { did: 'did:plc:carole2e', token: 'tok-carol' }
const sessions = new Map(
  [alice, bob, carol].map((p) => [`Bearer ${p.token}`, p.did]),
)
const passwords = new Map<string, string>()
let synapse = ''

async function registerUser(localpart: string) {
  const { nonce } = (await (
    await fetch(`${synapse}/_synapse/admin/v1/register`)
  ).json()) as { nonce: string }
  const password = randomUUID()
  const mac = createHmac('sha1', sharedSecret)
    .update([nonce, localpart, password, 'notadmin'].join('\0'))
    .digest('hex')
  const res = await fetch(`${synapse}/_synapse/admin/v1/register`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ nonce, username: localpart, password, mac }),
  })
  assert(res.ok, `register ${localpart}: ${res.status} ${await res.text()}`)
  passwords.set(localpart, password)
}

const localpartOf = (mxid: string) => mxid.slice(1, mxid.indexOf(':'))

try {
  // --- Synapse ---------------------------------------------------------------
  await writeFile(
    join(dir, 'homeserver.yaml'),
    JSON.stringify({
      server_name: serverName,
      public_baseurl: 'http://localhost:8008/',
      report_stats: false,
      pid_file: '/data/homeserver.pid',
      signing_key_path: '/data/signing.key',
      database: { name: 'sqlite3', args: { database: '/data/homeserver.db' } },
      media_store_path: '/data/media',
      trusted_key_servers: [],
      federation_domain_whitelist: [],
      registration_shared_secret: sharedSecret,
      rc_message: { per_second: 1000, burst_count: 1000 },
      rc_login: {
        address: { per_second: 1000, burst_count: 1000 },
        account: { per_second: 1000, burst_count: 1000 },
      },
      rc_registration: { per_second: 1000, burst_count: 1000 },
      listeners: [
        {
          port: 8008,
          tls: false,
          type: 'http',
          bind_addresses: ['0.0.0.0'],
          resources: [{ names: ['client'], compress: false }],
        },
      ],
      suppress_key_server_warning: true,
    }),
  )
  execFileSync(
    'docker',
    ['run', '--rm', '-d', '--name', container, '-p', '127.0.0.1::8008',
      '-v', `${dir}:/data`, SYNAPSE_IMAGE],
    { stdio: 'pipe' },
  )
  started = true
  synapse = `http://${execFileSync('docker', ['port', container, '8008'], { encoding: 'utf8' }).trim().split('\n')[0]}`
  for (let n = 0; n < 150; n++) {
    try {
      if ((await fetch(`${synapse}/_matrix/client/versions`)).ok) break
    } catch {}
    await delay(300)
  }
  step('synapse up', synapse)

  // --- Bridge (real router, fake M8 sessions, in-memory DB) ----------------
  const db = new SqliteBridgeDatabase({ dbPath: ':memory:' } as never)
  const log = { debug() {}, info() {}, warn(...a: unknown[]) { console.warn('[bridge warn]', ...a) }, error(...a: unknown[]) { console.error('[bridge error]', ...a) }, trace() {} }
  const ctx: any = {
    db,
    log,
    challenges: new ChallengeStore(),
    config: { port: 0, matrixServerName: serverName, m8BaseUrl: '' },
    matrix: {
      // The real bridge creates the requester's account through the
      // appservice; here the admin API does it.
      userExists: async (mxid: string) => passwords.has(localpartOf(mxid)),
      createUser: async (mxid: string) => registerUser(localpartOf(mxid)),
    },
    projection: { reconcileMemberAccess: async () => {} },
  }
  const server = createServer(async (req, res) => {
    try {
      if (req.url === '/sessions/me') {
        const did = sessions.get(req.headers.authorization ?? '')
        res.writeHead(did ? 200 : 401, { 'Content-Type': 'application/json' })
        res.end(JSON.stringify(did ? { session: { did } } : {}))
        return
      }
      if (!(await routeRequest(req, res, ctx))) {
        res.writeHead(404)
        res.end()
      }
    } catch (err: any) {
      res.writeHead(err?.status ?? err?.statusCode ?? 500, { 'Content-Type': 'application/json' })
      res.end(JSON.stringify({ error: String(err?.message ?? err) }))
    }
  })
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r))
  const bridge = `http://127.0.0.1:${(server.address() as any).port}`
  ctx.config.m8BaseUrl = bridge

  const call = async (who: { token: string }, path: string, body?: unknown) => {
    const res = await fetch(`${bridge}${path}`, {
      method: body === undefined ? 'GET' : 'POST',
      headers: { Authorization: `Bearer ${who.token}`, 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    })
    const json = await res.json().catch(() => ({}))
    return { status: res.status, json: json as any }
  }

  // Identities as iM8 would derive them; MXIDs follow from the identity key.
  const ids = {
    alice: deriveIdentity(new Uint8Array(32).fill(11), 'public'),
    bob: deriveIdentity(new Uint8Array(32).fill(22), 'public'),
    carol: deriveIdentity(new Uint8Array(32).fill(33), 'public'),
  }
  const mx = {
    alice: mxidForIdentityPub(ids.alice.pub, serverName) as string,
    bob: mxidForIdentityPub(ids.bob.pub, serverName) as string,
    carol: mxidForIdentityPub(ids.carol.pub, serverName) as string,
  }
  await registerUser(localpartOf(mx.bob))
  await registerUser(localpartOf(mx.carol))

  // --- 1. Alice asks Bob (dm.v1 proof, exactly PARA's body shape) ----------
  const { json: ch } = await call(alice, '/api/matrix-challenge', {})
  const signed = signAssertion(ids.alice, {
    purpose: 'matrix-login',
    audience: BRIDGE_AUDIENCES.dm,
    challenge: ch.challenge,
  })
  const created = await call(alice, '/api/dm-requests', { recipientDid: bob.did, ...signed })
  assert.equal(created.status, 201, JSON.stringify(created.json))
  const requestId = created.json.id as string
  assert(passwords.has(localpartOf(mx.alice)), 'bridge did not provision the requester account')
  step('alice asked bob', { requestId })

  // A replayed challenge is refused.
  const replay = await call(alice, '/api/dm-requests', { recipientDid: bob.did, ...signed })
  assert.equal(replay.status, 401)

  // --- 2. Bob sees it without Alice's MXID, accepts and gets it once -------
  const bobList = await call(bob, '/api/dm-requests')
  assert.equal(bobList.json.incoming.length, 1)
  assert.equal(bobList.json.incoming[0].requesterDid, alice.did)
  assert(!JSON.stringify(bobList.json).includes(mx.alice), 'list leaked the MXID')
  const accepted = await call(bob, '/api/dm-requests/accept', { id: requestId })
  assert.equal(accepted.status, 200, JSON.stringify(accepted.json))
  assert.equal(accepted.json.requesterMxid, mx.alice)
  const again = await call(bob, '/api/dm-requests/accept', { id: requestId })
  assert.equal(again.status, 404, 'MXID handed out twice')
  const aliceList = await call(alice, '/api/dm-requests')
  const outgoing = aliceList.json.outgoing
  assert.equal(outgoing[0].status, 'accepted')
  assert(!JSON.stringify(aliceList.json).includes(mx.alice))
  step('bob accepted; alice sees status accepted')

  // --- 3. Matrix clients with Rust crypto ----------------------------------
  async function startClient(mxid: string) {
    const login = (await (
      await fetch(`${synapse}/_matrix/client/v3/login`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          type: 'm.login.password',
          identifier: { type: 'm.id.user', user: localpartOf(mxid) },
          password: passwords.get(localpartOf(mxid)),
        }),
      })
    ).json()) as any
    assert(login.access_token, JSON.stringify(login))
    const client = sdk.createClient({
      baseUrl: synapse,
      userId: login.user_id,
      deviceId: login.device_id,
      accessToken: login.access_token,
      logger: sdk.logger?.getChild ? sdk.logger.getChild(mxid.slice(1, 6)) : undefined,
    })
    await client.initRustCrypto({ useIndexedDB: false })
    clients.push(client)
    await new Promise<void>((resolve) => {
      client.once(sdk.ClientEvent.Sync, () => resolve())
      client.startClient({ initialSyncLimit: 20 })
    })
    return client
  }
  ;(sdk.logger as any)?.setLevel?.('warn')
  const a = await startClient(mx.alice)
  const b = await startClient(mx.bob)
  const c = await startClient(mx.carol)
  step('matrix clients synced')

  async function waitFor<T>(what: string, fn: () => T | undefined | Promise<T | undefined>, ms = 30_000): Promise<T> {
    const end = Date.now() + ms
    while (Date.now() < end) {
      const v = await fn()
      if (v) return v
      await delay(250)
    }
    throw new Error(`timed out waiting for ${what}`)
  }

  // What the native client's createDirectRoom does: direct, trusted private,
  // encrypted from the start, history "invited", then the encrypted intro.
  async function createDirectRoom(client: any, invitee: string, intro: unknown) {
    const { room_id } = await client.createRoom({
      is_direct: true,
      preset: 'trusted_private_chat',
      invite: [invitee],
      initial_state: [
        { type: 'm.room.encryption', state_key: '', content: { algorithm: 'm.megolm.v1.aes-sha2' } },
        { type: 'm.room.history_visibility', state_key: '', content: { history_visibility: 'invited' } },
      ],
    })
    // Share the megolm key with the invitee: matrix-js-sdk (without lazy
    // members) encrypts for the members it already knows, so wait for the
    // invite. matrix-rust-sdk asks the server for /members before sharing.
    await waitFor('room encrypted with the invite known', () => {
      const room = client.getRoom(room_id)
      return (room?.hasEncryptionStateEvent() &&
        room.getMember(invitee)?.membership === 'invite') || undefined
    })
    if (intro) await client.sendEvent(room_id, 'm.room.message', intro)
    return room_id as string
  }

  // Bob: the accepted request's room.
  const roomId = await createDirectRoom(b, mx.alice, dmIntroMessage({ requestId, did: bob.did }))
  step('bob created the direct room and sent the intro', roomId)
  // Carol: a direct invite with a forged intro (right id, wrong DID) and one
  // with none. Alice must keep only Bob's.
  const forged = await createDirectRoom(c, mx.alice, dmIntroMessage({ requestId, did: carol.did }))
  const bare = await createDirectRoom(c, mx.alice, undefined)

  // --- 4. Alice: direct invites, join, decrypt, match ----------------------
  const invites = await waitFor('three direct invites', () => {
    const rooms = a.getRooms().filter((r: any) => r.getMyMembership() === 'invite' && r.getDMInviter())
    return rooms.length === 3 ? rooms : undefined
  })
  assert.deepEqual(new Set(invites.map((r: any) => r.roomId)), new Set([roomId, forged, bare]))
  step('alice sees 3 direct invites (is_direct on the invite)')

  async function readIntro(id: string) {
    await a.joinRoom(id)
    return waitFor(`intro in ${id}`, async () => {
      const events = a.getRoom(id)?.getLiveTimeline().getEvents() ?? []
      for (const ev of events) {
        await a.decryptEventIfNeeded(ev)
        if (ev.getType() === 'm.room.message' && ev.getContent()?.msgtype === DM_INTRO_MSGTYPE) {
          return { content: ev.getContent(), encrypted: ev.isEncrypted(), failed: ev.isDecryptionFailure() }
        }
      }
      return undefined
    }, 15_000).catch(() => {
      const evs = a.getRoom(id)?.getLiveTimeline().getEvents() ?? []
      step(`no intro in ${id}`, evs.map((e: any) => `${e.getType()}${e.isDecryptionFailure() ? '(UTD)' : ''}`))
      return undefined
    })
  }

  const verdicts: Record<string, string> = {}
  for (const id of [roomId, forged, bare]) {
    const intro = await readIntro(id)
    if (intro) assert(intro.encrypted && !intro.failed, 'intro was not an encrypted, decrypted event')
    // As in the app: mapTimeline parses the body, reconcile matches it.
    const parsed = intro ? parseDmIntroBody(intro.content.body) : undefined
    const match = parsed ? matchDmIntro(parsed, outgoing) : undefined
    if (match) {
      verdicts[id] = `kept (${match.recipientDid})`
    } else {
      await a.leave(id)
      verdicts[id] = intro ? 'left: intro does not match' : 'left: no intro'
    }
  }
  step('alice verdicts', verdicts)
  assert.match(verdicts[roomId], /^kept \(did:plc:bobe2e\)$/)
  assert.equal(verdicts[forged], 'left: intro does not match')
  assert.equal(verdicts[bare], 'left: no intro')

  // --- 5. Conversation both ways, still encrypted ---------------------------
  await a.sendEvent(roomId, 'm.room.message', { msgtype: 'm.text', body: 'hola bob' })
  const got = await waitFor('bob reads alice', async () => {
    for (const ev of b.getRoom(roomId)?.getLiveTimeline().getEvents() ?? []) {
      await b.decryptEventIfNeeded(ev)
      if (ev.getContent()?.body === 'hola bob') return ev
    }
    return undefined
  })
  assert(got.isEncrypted())
  step('bob decrypted alice\'s reply')

  // --- 6. Alice dismisses the verified request -------------------------------
  const dismissed = await call(alice, '/api/dm-requests/decline', { id: requestId })
  assert.equal(dismissed.status, 200, JSON.stringify(dismissed.json))
  const after = await call(alice, '/api/dm-requests')
  assert.equal(after.json.outgoing.length, 0)
  step('request removed from the bridge')

  server.close()
  await db.close()
  step('PASS')
} finally {
  for (const client of clients) client.stopClient()
  if (started) execFileSync('docker', ['rm', '-f', container], { stdio: 'pipe' })
  await rm(dir, { recursive: true, force: true })
}
process.exit(0)
