// Synthetic wallet only. Run with tsx from the sibling iM8 checkout.
import assert from 'node:assert/strict'
import { randomBytes, createHash, randomUUID } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync, writeFileSync, renameSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { signIdentityChallenge } from '../../../iM8/src/services/identitySignature.ts'

const state = fileURLToPath(new URL('../.dev/chat-staging/', import.meta.url))
const manifest = JSON.parse(readFileSync(`${state}manifest.json`, 'utf8'))
assert.equal(manifest.project, 'para-chat-staging')
const pds = 'http://localhost:32583'
const appview = 'http://localhost:32584'
const bridge = 'http://localhost:33001'
const m8 = 'http://localhost:38787/v1'
const idp = manifest.publicUrls?.idp ?? 'http://localhost:38090'
const fixtureFile = `${state}fixtures.json`
const fixture = existsSync(fixtureFile) ? JSON.parse(readFileSync(fixtureFile, 'utf8')) : {
  handle: `chat${Date.now()}.test`, password: randomBytes(24).toString('base64url'),
  seed: randomBytes(32).toString('hex'),
}
const save = () => {
  const temporary = `${fixtureFile}.${process.pid}.tmp`
  writeFileSync(temporary, JSON.stringify(fixture, null, 2), { mode: 0o600 })
  renameSync(temporary, fixtureFile)
}
save()
const seed = new Uint8Array(Buffer.from(fixture.seed, 'hex'))
const signed = (challenge: string, audience: string) => signIdentityChallenge(seed, 'public', {
  purpose: 'matrix-login', challenge, audience,
})

async function request(base: string, path: string, body?: unknown, token?: string, expected = 200) {
  const response = await fetch(`${base}${path}`, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { ...(body === undefined ? {} : { 'content-type': 'application/json' }),
      ...(token ? { authorization: `Bearer ${token}` } : {}),
      ...(base === pds ? { 'atproto-proxy': 'did:web:localhost%3A32584#bsky_appview' } : {}),
    }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(60000),
  })
  const value = await response.json()
  assert.equal(response.status, expected, `${path}: HTTP ${response.status}, ${value.error ?? value.message ?? ''}`)
  return value
}
async function eventually(job: () => Promise<boolean>, label: string) {
  for (let n = 0; n < 60; n++) {
    try { if (await job()) return } catch {}
    await new Promise(resolve => setTimeout(resolve, 1000))
  }
  throw new Error(`Staging timeout: ${label}`)
}
async function proofCall(path: string, audience: string, extra = {}) {
  const { challenge } = await request(bridge, '/api/matrix-challenge', {}, fixture.m8Token)
  return request(bridge, path, { assertion: signed(challenge, audience), ...extra }, fixture.m8Token)
}

async function loginMas(deviceId = 'CHAT_STAGING_DEVICE', persist = true) {
  const mas = manifest.publicUrls?.mas ?? 'http://localhost:38080'
  const redirectUri = 'social.para.app:/matrix-auth'
  if (!fixture.masClientId) {
    const registered = await request(mas, '/oauth2/registration', { client_name: 'PARA Staging',
      client_uri: 'https://para.social', application_type: 'native', redirect_uris: [redirectUri],
      grant_types: ['authorization_code', 'refresh_token'], response_types: ['code'], token_endpoint_auth_method: 'none' }, undefined, 201)
    fixture.masClientId = registered.client_id
    save()
  }
  const verifier = randomBytes(32).toString('base64url')
  const expectedState = randomBytes(16).toString('hex')
  const start = new URL(`${mas}/authorize`)
  start.search = new URLSearchParams({ client_id: fixture.masClientId, redirect_uri: redirectUri,
    response_type: 'code', scope: `openid urn:matrix:org.matrix.msc2967.client:api:* urn:matrix:org.matrix.msc2967.client:device:${deviceId}`,
    code_challenge: createHash('sha256').update(verifier).digest('base64url'), code_challenge_method: 'S256',
    state: expectedState, nonce: randomBytes(16).toString('hex') }).toString()
  const cookies = new Map<string, string>()
  let next = start.toString()
  let form: URLSearchParams | undefined
  for (let n = 0; n < 20; n++) {
    const url = new URL(next)
    if (url.protocol === 'social.para.app:') {
      assert.equal(`${url.protocol}${url.pathname}`, redirectUri)
      assert.equal(url.searchParams.get('state'), expectedState)
      assert(url.searchParams.get('code'), 'MAS must return an authorization code')
      const response = await fetch(`${mas}/oauth2/token`, { method: 'POST',
        headers: { 'content-type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({ grant_type: 'authorization_code', client_id: fixture.masClientId,
          redirect_uri: redirectUri, code_verifier: verifier, code: url.searchParams.get('code')! }) })
      assert.equal(response.status, 200, 'MAS code redemption')
      const tokens = await response.json()
      const who = await request('http://localhost:38008', '/_matrix/client/v3/account/whoami', undefined, tokens.access_token)
      assert.equal(who.user_id, fixture.matrixUserId)
      assert.equal(who.device_id, deviceId)
      if (persist) {
        fixture.matrixToken = tokens.access_token
        fixture.matrixRefreshToken = tokens.refresh_token
        fixture.matrixDeviceId = deviceId
        save()
      }
      return { ...who, accessToken: tokens.access_token, refreshToken: tokens.refresh_token }
    }
    assert([mas, idp].includes(url.origin), 'Authorization may only visit the staging MAS and IdP')
    if (url.origin === idp) {
      assert.equal(url.pathname, '/authorize')
      const response = await fetch(url, { headers: { accept: 'application/json' } })
      assert.equal(response.status, 200)
      const { challenge } = await response.json()
      const mailbox = await request(m8, '/matrix/sign-requests', { challenge, audience: 'para-idp', issuer: idp }, fixture.m8Token, 201)
      await request(m8, `/matrix/sign-requests/${mailbox.id}/fulfill`, signed(challenge, 'para-idp'), fixture.m8Token)
      const fulfilled = await request(m8, `/matrix/sign-requests/${mailbox.id}`, undefined, fixture.m8Token)
      const approved = await request(idp, '/authorize/verify', { challenge, ...fulfilled.assertion })
      assert.equal(new URL(approved.redirect_uri).origin, mas)
      next = approved.redirect_uri
      continue
    }
    const response = await fetch(url, { method: form ? 'POST' : 'GET',
      headers: { cookie: [...cookies].map(([key, value]) => `${key}=${value}`).join('; '),
        ...(form ? { 'content-type': 'application/x-www-form-urlencoded' } : {}) },
      body: form, redirect: 'manual' })
    form = undefined
    for (const cookie of response.headers.getSetCookie()) {
      const part = cookie.split(';')[0]
      const separator = part.indexOf('=')
      cookies.set(part.slice(0, separator), part.slice(separator + 1))
    }
    if ([302, 303, 307].includes(response.status)) {
      next = new URL(response.headers.get('location')!, url).toString()
      continue
    }
    const html = await response.text()
    writeFileSync(`${state}mas-acceptance.html`, html, { mode: 0o600 })
    if (response.status === 200 && url.pathname.startsWith('/upstream/link/')) {
      const csrf = /name="csrf"\s+value="([^"]+)"/.exec(html)?.[1]
      if (csrf && html.includes('name="action" value="register"')) {
        assert(html.includes(fixture.matrixUserId), 'MAS must force the derived Matrix localpart')
        form = new URLSearchParams({ csrf, action: 'register' })
        continue
      }
    }
    if (response.status === 200 && /^\/register\/steps\/[^/]+\/display-name$/.test(url.pathname)) {
      const csrf = /name="csrf"\s+value="([^"]+)"/.exec(html)?.[1]
      assert(csrf && html.includes('name="action" value="skip"'), 'Display name must remain optional')
      form = new URLSearchParams({ csrf, action: 'skip' })
      continue
    }
    if (response.status === 200 && /^\/consent\/[^/]+$/.test(url.pathname)) {
      const csrf = /name="csrf"\s+value="([^"]+)"/.exec(html)?.[1]
      assert(csrf && html.includes('PARA Staging'), 'Consent must identify the synthetic test client')
      form = new URLSearchParams({ csrf })
      continue
    }
    throw new Error(`MAS acceptance requires handling the rendered page at ${url.pathname}; private HTML saved`)
  }
  throw new Error('MAS authorization exceeded the redirect bound')
}

try {
  if (process.argv.includes('--expect-restored')) {
    assert(fixture.matrixRefreshToken && fixture.m8RefreshToken)
    const refresh = await fetch('http://localhost:38080/oauth2/token', { method: 'POST',
      headers: { 'content-type': 'application/x-www-form-urlencoded' },
      body: new URLSearchParams({ grant_type: 'refresh_token', client_id: fixture.masClientId, refresh_token: fixture.matrixRefreshToken }),
      signal: AbortSignal.timeout(10000) })
    assert.equal(refresh.status, 200, 'Restored MAS session must refresh')
    const tokens = await refresh.json()
    fixture.matrixToken = tokens.access_token
    fixture.matrixRefreshToken = tokens.refresh_token
    const prior = await request('http://localhost:38008', '/_matrix/client/v3/account/whoami', undefined, fixture.matrixToken)
    assert.equal(prior.user_id, fixture.matrixUserId)
    assert.equal(prior.device_id, fixture.matrixDeviceId)
    const renewed = await request(m8, '/sessions/refresh', { refreshToken: fixture.m8RefreshToken })
    fixture.m8Token = renewed.accessToken
    fixture.m8RefreshToken = renewed.refreshToken
    save()
    await request(m8, '/sessions/me', undefined, fixture.m8Token)
    console.log('PASS existing MAS and M8 sessions survive the coordinated restore')
  }
  if (process.argv.includes('--ops') && fixture.opsEvidence) {
    const password = readFileSync(`${state}secrets/console_staging_password`, 'utf8').trim()
    const response = await fetch('http://localhost:38000/chat-ops/v1/status', {
      headers: { authorization: `Basic ${Buffer.from(`staging:${password}`).toString('base64')}` }, signal: AbortSignal.timeout(10000),
    })
    assert.equal(response.status, 200)
    const status = await response.json()
    assert.equal(status.jobs.find((job: any) => job.id === fixture.opsEvidence.jobId)?.state, 'done')
    assert.equal(status.audit.find((entry: any) => entry.requestId === fixture.opsEvidence.retryRequestId)?.result, 'success')
    assert.equal(status.communities.find((community: any) => community.communityUri === fixture.opsEvidence.communityUri)?.spaceId, fixture.opsEvidence.spaceId)
    console.log('PASS provisioning job, room identity and operator audit survive restart/restore')
  }
  const metadata = await request(idp, '/.well-known/openid-configuration')
  assert.equal(new URL(metadata.jwks_uri).origin, idp)
  const jwks = await request(idp, new URL(metadata.jwks_uri).pathname)
  const keyHash = createHash('sha256').update(JSON.stringify(jwks.keys.map(({ kid, kty, n, e, x, crv }: any) => ({ kid, kty, n, e, x, crv })))).digest('hex')
  if (fixture.idpKeyFingerprint) assert.equal(keyHash, fixture.idpKeyFingerprint, 'IdP signing keys changed')
  fixture.idpKeyFingerprint = keyHash
  save()
  const account = fixture.did
    ? await request(pds, '/xrpc/com.atproto.server.createSession', { identifier: fixture.handle, password: fixture.password })
    : await request(pds, '/xrpc/com.atproto.server.createAccount', { handle: fixture.handle,
        email: `${fixture.handle}@example.invalid`, password: fixture.password })
  if (fixture.did) assert.equal(account.did, fixture.did, 'Account DID changed after restart/restore')
  fixture.did = account.did
  fixture.pdsToken = account.accessJwt
  save()
  console.log('PASS persistent native PDS account')
  if (!fixture.postUri) {
    const post = await request(pds, '/xrpc/com.atproto.repo.createRecord', { repo: account.did,
      collection: 'app.bsky.feed.post', record: { $type: 'app.bsky.feed.post',
        text: `Persistent chat staging ${fixture.handle}`, createdAt: new Date().toISOString() } }, account.accessJwt)
    fixture.postUri = post.uri
    fixture.postCid = post.cid
    save()
  }
  await eventually(async () => {
    // A persistent fixture can outlive the first page of its author's feed.
    const result = await request(appview, `/xrpc/app.bsky.feed.getPosts?uris=${encodeURIComponent(fixture.postUri)}`)
    return result.posts.some((entry: any) => entry.uri === fixture.postUri && entry.cid === fixture.postCid)
  }, 'VLPDS → rsky → AppView')
  // A preserved old feed alone cannot prove ingestion resumed after recovery.
  const live = await request(pds, '/xrpc/com.atproto.repo.createRecord', { repo: account.did,
    collection: 'app.bsky.feed.post', record: { $type: 'app.bsky.feed.post',
      text: `Fresh staging probe ${randomBytes(8).toString('hex')}`, createdAt: new Date().toISOString() } }, account.accessJwt)
  await eventually(async () => {
    const feed = await request(appview, `/xrpc/app.bsky.feed.getAuthorFeed?actor=${account.did}`)
    return feed.feed.some((entry: any) => entry.post.uri === live.uri && entry.post.cid === live.cid)
  }, 'fresh commit after recovery')
  const proxy = await request(pds, `/xrpc/app.bsky.feed.getAuthorFeed?actor=${account.did}`, undefined, account.accessJwt)
  assert(proxy.feed.some((entry: any) => entry.post.uri === live.uri))
  console.log('PASS VLPDS → rsky → AppView → PDS proxy with matching CID')
  if (!fixture.boardUri) {
    const board = await request(pds, '/xrpc/com.para.community.createBoard', {
      name: `Chat staging ${fixture.handle}-${fixture.communityVersion ?? 1}`, quadrant: 'MX-NL',
    }, account.accessJwt)
    fixture.boardUri = board.uri
    fixture.boardCid = board.cid
    save()
  }
  const membership = await request(pds, `/xrpc/com.atproto.repo.getRecord?repo=${account.did}&collection=com.para.community.membership&rkey=${fixture.boardUri.split('/').at(-1)}`)
  assert.equal(membership.value.membershipState, 'active')
  await request(pds, '/xrpc/com.atproto.repo.createRecord', { repo: account.did,
    collection: 'com.para.community.vote', validate: false, record: {} }, account.accessJwt, 400)
  console.log('PASS native community, creator membership and validate=false policy enforcement')
  const session = await request(m8, '/sessions/start', { identifier: account.did })
  const sessionId = session.session?.sessionId ?? session.attempt?.sessionId
  assert(sessionId)
  if (fixture.m8SessionId) assert.equal(sessionId, fixture.m8SessionId, 'M8 session identity changed')
  fixture.m8SessionId = sessionId
  fixture.m8Token = session.tokens.accessToken
  fixture.m8RefreshToken = session.tokens.refreshToken
  save()
  const identity = await proofCall('/api/matrix-identity', 'para-matrix-bridge/identity.v1')
  if (fixture.matrixUserId) assert.equal(identity.userId, fixture.matrixUserId)
  fixture.matrixUserId = identity.userId
  save()
  assert.equal(identity.loginFlow, 'oidc')
  assert.equal(identity.homeServer, manifest.publicUrls?.matrix ?? 'http://localhost:38008')
  console.log('PASS M8 session and wallet-derived Matrix identity through real bridge')
  const verifier = randomBytes(32).toString('base64url')
  const authorization = new URL(`${idp}/authorize`)
  authorization.search = new URLSearchParams({ client_id: 'mas', response_type: 'code',
    redirect_uri: `${manifest.publicUrls?.mas ?? 'http://localhost:38080'}/upstream/callback/01K3ZQD8N7P0J5R2T9V4W6X8YB`,
    scope: 'openid', state: randomBytes(16).toString('hex'), nonce: randomBytes(16).toString('hex'),
    code_challenge: createHash('sha256').update(verifier).digest('base64url'), code_challenge_method: 'S256' }).toString()
  const challengeResponse = await fetch(authorization, { headers: { accept: 'application/json' } })
  assert.equal(challengeResponse.status, 200)
  const { challenge } = await challengeResponse.json()
  const mailbox = await request(m8, '/matrix/sign-requests', { challenge, audience: 'para-idp', issuer: idp }, fixture.m8Token, 201)
  const proof = signed(challenge, 'para-idp')
  await request(m8, `/matrix/sign-requests/${mailbox.id}/fulfill`, proof, fixture.m8Token)
  const fulfilled = await request(m8, `/matrix/sign-requests/${mailbox.id}`, undefined, fixture.m8Token)
  assert.equal(fulfilled.status, 'fulfilled')
  const approved = await request(idp, '/authorize/verify', { challenge, ...fulfilled.assertion })
  assert.equal(new URL(approved.redirect_uri).origin, manifest.publicUrls?.mas ?? 'http://localhost:38080')
  await request(idp, '/authorize/verify', { challenge, ...proof }, undefined, 400)
  console.log('PASS real iM8 signer → M8 mailbox → IdP approval, issuer pinning and replay rejection')
  if (process.argv.includes('--mas')) {
    const who = await loginMas()
    console.log('PASS MAS native registration → IdP → authorization code + PKCE → actual Matrix device')
    const absentChallenge = await request(bridge, '/api/matrix-challenge', {}, fixture.m8Token)
    const absent = await request(bridge, '/api/matrix-attest', {
      assertion: signed(absentChallenge.challenge, 'para-matrix-bridge/attest.v1'), deviceId: `ABSENT_${randomBytes(8).toString('hex')}`,
    }, fixture.m8Token, 409)
    assert.equal(absent.error, 'MATRIX_DEVICE_NOT_FOUND')
    console.log('PASS bridge refuses a fabricated SDK device before creating its attestation')
    const attested = await proofCall('/api/matrix-attest', 'para-matrix-bridge/attest.v1', { deviceId: who.device_id, friendlyName: 'Staging OAuth device' })
    assert.equal(attested.deviceId, who.device_id)
    assert.equal(attested.attested, true)
    const joined = await proofCall('/api/community-join', 'para-matrix-bridge/join.v1', { communityUri: fixture.boardUri })
    assert.equal(joined.userId, who.user_id)
    assert(joined.joinedRoomIds.length)
    if (fixture.spaceId) assert.equal(joined.spaceId, fixture.spaceId, 'Room identity changed after recovery')
    fixture.spaceId = joined.spaceId
    fixture.joinedRoomIds = joined.joinedRoomIds
    save()
    console.log('PASS actual-device attestation and verified community room admission')
    if (process.argv.includes('--revoke')) {
      const disposable = await loginMas(`STAGING_REVOKE_${randomBytes(6).toString('hex')}`, false)
      assert(disposable.refreshToken, 'MAS must issue a refresh token for the native client')
      const refresh = (token: string) => fetch('http://localhost:38080/oauth2/token', {
        method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({ grant_type: 'refresh_token', client_id: fixture.masClientId, refresh_token: token }),
        signal: AbortSignal.timeout(10000),
      })
      const renewed = await refresh(disposable.refreshToken)
      assert.equal(renewed.status, 200, 'Active MAS device must refresh')
      const tokens = await renewed.json()
      await request('http://localhost:38008', '/_matrix/client/v3/account/whoami', undefined, tokens.access_token)
      const device = await proofCall('/api/matrix-attest', 'para-matrix-bridge/attest.v1', {
        deviceId: disposable.device_id, friendlyName: 'Synthetic revocation check',
      })
      if (process.argv.includes('--ops')) {
        const password = readFileSync(`${state}secrets/console_staging_password`, 'utf8').trim()
        const operationId = randomUUID()
        const opsResponse = await fetch(`http://localhost:38000/chat-ops/v1/devices/${device.sessionId}/revoke`, {
          method: 'POST', headers: { authorization: `Basic ${Buffer.from(`staging:${password}`).toString('base64')}`,
            origin: 'http://localhost:38000', 'content-type': 'application/json', 'x-para-chat-action': '1' },
          body: JSON.stringify({ requestId: operationId, reason: 'Synthetic native device revocation acceptance' }),
          signal: AbortSignal.timeout(60000),
        })
        assert.equal(opsResponse.status, 200)
        const result = await opsResponse.json()
        assert.equal(result.result, 'success')
        assert.equal(result.requestId, operationId)
        console.log('PASS private operator gateway authorizes and audits native device revocation')
      } else {
        await request(bridge, '/api/devices/revoke', { sessionId: device.sessionId }, fixture.m8Token)
      }
      await request('http://localhost:38008', '/_matrix/client/v3/account/whoami', undefined, disposable.accessToken, 401)
      await request('http://localhost:38008', '/_matrix/client/v3/account/whoami', undefined, tokens.access_token, 401)
      assert.equal((await refresh(tokens.refresh_token)).status, 400, 'Revoked MAS device must not refresh')
      console.log('PASS active MAS token refresh; revocation invalidates access and refresh tokens')
    }
    if (process.argv.includes('--crypto')) {
      const { runCryptoAcceptance } = await import('./crypto.mts')
      await runCryptoAcceptance(fixture, save, id => loginMas(id, false))
    }
  }
  const checkpoint = JSON.parse(execFileSync('docker', ['compose', '-p', 'para-chat-staging', '-f', `${state}compose.json`,
    'exec', '-T', 'postgres', 'psql', '-XAt', '-U', 'pg', '-d', 'bridge', '-c',
    "SELECT json_build_object('cursor',(SELECT cursor::text FROM sync_cursor WHERE id=1),'sourceId',(SELECT source_id FROM sync_source WHERE id=1))"],
    { encoding: 'utf8', timeout: 10000 }))
  assert.equal(checkpoint.sourceId, manifest.streamId)
  assert(checkpoint.cursor)
  if (fixture.bridgeCursor) assert(BigInt(checkpoint.cursor) >= BigInt(fixture.bridgeCursor), 'Bridge checkpoint regressed')
  fixture.bridgeCursor = checkpoint.cursor
  save()
  console.log('PASS durable source-bound bridge checkpoint and stable IdP signing keys')
  console.log(process.argv.includes('--crypto')
    ? 'PASS automated staging acceptance; physical app and reviewed migration remain separate gates'
    : 'PASS protocol staging acceptance; run --mas --crypto for device encryption and key recovery')
} finally {
  seed.fill(0)
}

// Completed SDK SAS verification retains expiry timers after stopClient(). This
// standalone acceptance CLI has no service work to keep alive after all checks.
process.stdout.write('', () => process.exit(0))
