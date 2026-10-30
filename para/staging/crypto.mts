// Real MAS device sessions and real Rust crypto; no administrator login.
import assert from 'node:assert/strict'
import { createHash, randomBytes } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { setTimeout as delay } from 'node:timers/promises'
import * as sdk from '../../../PARA/node_modules/matrix-js-sdk/lib/index.js'
import type { Logger } from '../../../PARA/node_modules/matrix-js-sdk/lib/logger.js'
import { CryptoEvent } from '../../../PARA/node_modules/matrix-js-sdk/lib/crypto-api/CryptoEvent.js'
import { VerificationPhase } from '../../../PARA/node_modules/matrix-js-sdk/lib/crypto-api/verification.js'

export async function runCryptoAcceptance(
  fixture: any, save: () => void,
  login: (deviceId: string) => Promise<{ user_id: string; device_id: string; accessToken: string }>,
) {
  const manifest = JSON.parse(readFileSync(new URL('../.dev/chat-staging/manifest.json', import.meta.url), 'utf8'))
  const baseUrl = manifest.publicUrls?.matrix ?? 'http://localhost:38008'
  const require = createRequire(new URL('../../../PARA/node_modules/matrix-js-sdk/package.json', import.meta.url))
  const attachments = require('@matrix-org/matrix-sdk-crypto-wasm')
  await attachments.initAsync()
  const logger: Logger = { trace() {}, debug() {}, info() {}, warn: console.warn, error: console.error,
    getChild() { return this } }
  const clients: ReturnType<typeof sdk.createClient>[] = []
  const suffix = randomBytes(6).toString('hex')
  async function until<T>(label: string, job: () => Promise<T | undefined> | T | undefined): Promise<T> {
    const deadline = Date.now() + 60000
    while (Date.now() < deadline) {
      const value = await job()
      if (value !== undefined && value !== false) return value
      await delay(250)
    }
    throw new Error(`Crypto acceptance timed out: ${label}`)
  }
  async function start(label: string) {
    const session = await login(`STAGING_${label}_${suffix}`)
    const client = sdk.createClient({ baseUrl, userId: session.user_id,
      deviceId: session.device_id, accessToken: session.accessToken, logger })
    clients.push(client)
    await client.initRustCrypto({ useIndexedDB: false })
    await client.startClient({ initialSyncLimit: 30 })
    await until('initial sync', () => client.getSyncState() === 'PREPARED' || client.getSyncState() === 'SYNCING' ? true : undefined)
    return client
  }
  async function plaintext(client: ReturnType<typeof sdk.createClient>, roomId: string, eventId: string) {
    return until('decrypt remote event', async () => {
      const event = client.getRoom(roomId)?.getLiveTimeline().getEvents().find(event => event.getId() === eventId)
      if (!event) return undefined
      await client.decryptEventIfNeeded(event)
      return event.getType() === 'm.room.message' && !event.isDecryptionFailure() ? event : undefined
    })
  }
  async function recoveredMedia(client: ReturnType<typeof sdk.createClient>, roomId: string, eventId: string, expectedHash: string) {
    const event = await plaintext(client, roomId, eventId)
    const file = event.getContent().file
    const mxc = new URL(file.url)
    const response = await fetch(`${baseUrl}/_matrix/client/v1/media/download/${encodeURIComponent(mxc.hostname)}${mxc.pathname}`,
      { headers: { authorization: `Bearer ${client.getAccessToken()}` }, signal: AbortSignal.timeout(15000) })
    assert.equal(response.status, 200)
    const attachment = new attachments.EncryptedAttachment(new Uint8Array(await response.arrayBuffer()),
      JSON.stringify({ v: file.v, key: file.key, iv: file.iv, hashes: file.hashes }))
    try { assert.equal(createHash('sha256').update(attachments.Attachment.decrypt(attachment)).digest('hex'), expectedHash) }
    finally { attachment.free() }
  }
  try {
    const a = await start('A')
    const prior = fixture.cryptoEvidence
    if (prior) {
      const priorKey = new Uint8Array(Buffer.from(prior.backupKey, 'base64'))
      await a.getCrypto()!.storeSessionBackupPrivateKey(priorKey, prior.backupVersion)
      await a.getCrypto()!.restoreKeyBackup()
      assert.equal((await plaintext(a, prior.roomId, prior.eventId)).getContent().body, prior.message)
      if (prior.mediaEventId) {
        await recoveredMedia(a, prior.roomId, prior.mediaEventId, prior.mediaPlaintextHash)
        console.log('PASS prior encrypted attachment survives coordinated recovery')
      }
      priorKey.fill(0)
      console.log('PASS prior encrypted history and backup keys survive the staging restart/restore')
    }
    const b = await start('B')
    const { room_id: roomId } = await a.createRoom({ name: 'Synthetic staging crypto', preset: sdk.Preset.PrivateChat,
      initial_state: [
        { type: 'm.room.encryption', state_key: '', content: { algorithm: 'm.megolm.v1.aes-sha2' } },
        { type: 'm.room.history_visibility', state_key: '', content: { history_visibility: 'shared' } },
      ] })
    await until('both devices know encryption', () => a.getRoom(roomId)?.hasEncryptionStateEvent() && b.getRoom(roomId)?.hasEncryptionStateEvent() ? true : undefined)
    const message = `Synthetic encrypted staging ${suffix}`
    const sent = await a.sendEvent(roomId, sdk.EventType.RoomMessage, { msgtype: sdk.MsgType.Text, body: message })
    const raw = await fetch(`http://localhost:38008/_matrix/client/v3/rooms/${encodeURIComponent(roomId)}/event/${encodeURIComponent(sent.event_id)}`,
      { headers: { authorization: `Bearer ${a.getAccessToken()}` } })
    assert.equal(raw.status, 200)
    const stored = await raw.json()
    assert.equal(stored.type, 'm.room.encrypted')
    assert(!JSON.stringify(stored).includes(message), 'Server must only store ciphertext')
    const received = await plaintext(b, roomId, sent.event_id)
    assert(received.isEncrypted())
    assert.equal(received.getContent().body, message)
    const reply = await b.sendEvent(roomId, sdk.EventType.RoomMessage, { msgtype: sdk.MsgType.Text, body: `${message} reply`,
      'm.relates_to': { 'm.in_reply_to': { event_id: sent.event_id } } })
    const replyEvent = await plaintext(a, roomId, reply.event_id)
    assert.equal(replyEvent.getContent().body, `${message} reply`)
    assert.equal(replyEvent.getContent()['m.relates_to']['m.in_reply_to'].event_id, sent.event_id)
    console.log('PASS two genuine MAS devices encrypt/decrypt both directions; homeserver stores ciphertext')

    const original = new TextEncoder().encode(`Synthetic encrypted attachment ${suffix}`)
    const encrypted = attachments.Attachment.encrypt(original)
    let mediaEventId: string
    try {
      const ciphertext = encrypted.encryptedData
      const info = JSON.parse(encrypted.mediaEncryptionInfo)
      const uploaded = await a.uploadContent(Buffer.from(ciphertext), { type: 'application/octet-stream', includeFilename: false })
      const mediaEvent = await a.sendEvent(roomId, sdk.EventType.RoomMessage, {
        msgtype: sdk.MsgType.File, body: 'synthetic-attachment.txt',
        file: { ...info, url: uploaded.content_uri }, info: { mimetype: 'text/plain', size: original.length },
      })
      mediaEventId = mediaEvent.event_id
      const receivedMedia = await plaintext(b, roomId, mediaEvent.event_id)
      const file = receivedMedia.getContent().file
      const mxc = new URL(file.url)
      const response = await fetch(`${baseUrl}/_matrix/client/v1/media/download/${encodeURIComponent(mxc.hostname)}${mxc.pathname}`,
        { headers: { authorization: `Bearer ${b.getAccessToken()}` }, signal: AbortSignal.timeout(15000) })
      assert.equal(response.status, 200)
      const downloaded = new Uint8Array(await response.arrayBuffer())
      assert.deepEqual(downloaded, ciphertext, 'Server must store attachment ciphertext')
      assert.notDeepEqual(downloaded, original)
      const metadata = JSON.stringify({ v: file.v, key: file.key, iv: file.iv, hashes: file.hashes })
      const attachment = new attachments.EncryptedAttachment(downloaded, metadata)
      try { assert.deepEqual(attachments.Attachment.decrypt(attachment), original) } finally { attachment.free() }
      const tampered = downloaded.slice()
      tampered[0] ^= 1
      const corrupt = new attachments.EncryptedAttachment(tampered, metadata)
      try { assert.throws(() => attachments.Attachment.decrypt(corrupt)) } finally { corrupt.free() }
      console.log('PASS authenticated encrypted media upload/download, peer decryption and tamper rejection')
    } finally { encrypted.free() }

    let incoming: any
    let verificationError: unknown
    b.on(CryptoEvent.VerificationRequestReceived, request => {
      incoming = request
      void request.accept().catch(error => { verificationError = error })
    })
    const outgoing = await a.getCrypto()!.requestDeviceVerification(fixture.matrixUserId, b.getDeviceId()!)
    await until('SAS request accepted', () => {
      if (verificationError) throw verificationError
      return incoming?.phase === VerificationPhase.Ready && outgoing.phase === VerificationPhase.Ready ? true : undefined
    })
    const verifierA = await outgoing.startVerification('m.sas.v1')
    const doneA = verifierA.verify()
    void doneA.catch(error => { verificationError = error })
    const verifierB: any = await until('peer SAS verifier', () => incoming.verifier ?? undefined)
    const doneB = verifierB.verify()
    void doneB.catch((error: unknown) => { verificationError = error })
    const sasA: any = await until('initiator SAS', () => {
      if (verificationError) throw verificationError
      return verifierA.getShowSasCallbacks() ?? undefined
    })
    const sasB: any = await until('recipient SAS', () => verifierB.getShowSasCallbacks() ?? undefined)
    assert.deepEqual(sasA.sas.decimal, sasB.sas.decimal, 'SAS numbers must independently match')
    await Promise.all([sasA.confirm(), sasB.confirm()])
    await Promise.all([doneA, doneB])
    assert((await a.getCrypto()!.getDeviceVerificationStatus(fixture.matrixUserId, b.getDeviceId()!))?.isVerified())
    console.log('PASS both MAS devices compare matching SAS and complete cryptographic verification')

    const crypto = a.getCrypto()!
    await crypto.resetKeyBackup()
    const version = await until('active encrypted key backup', () => crypto.getActiveSessionBackupVersion().then(version => version ?? undefined))
    const key = await crypto.getSessionBackupPrivateKey()
    assert(key)
    // Wait for the real server to receive a key backup for this conversation.
    await until('server receives Megolm backup', async () => {
      const response = await fetch(`http://localhost:38008/_matrix/client/v3/room_keys/keys?version=${version}`,
        { headers: { authorization: `Bearer ${a.getAccessToken()}` } })
      if (!response.ok) return undefined
      const data = await response.json()
      return Object.keys(data.rooms?.[roomId]?.sessions ?? {}).length &&
        (!prior || Object.keys(data.rooms?.[prior.roomId]?.sessions ?? {}).length) ? true : undefined
    })
    if (prior) (fixture.cryptoEvidenceHistory ??= []).push(prior)
    fixture.cryptoEvidence = { roomId, eventId: sent.event_id, replyId: reply.event_id,
      mediaEventId, mediaPlaintextHash: createHash('sha256').update(original).digest('hex'),
      message, backupVersion: version, backupKey: Buffer.from(key).toString('base64') }
    save()
    const c = await start('RECOVERY')
    const wrongKey = new Uint8Array(randomBytes(32))
    await c.getCrypto()!.storeSessionBackupPrivateKey(wrongKey, version)
    await assert.rejects(c.getCrypto()!.restoreKeyBackup(), /key|backup/i)
    wrongKey.fill(0)
    console.log('PASS recovery refuses an incorrect backup key before importing history')
    await c.getCrypto()!.storeSessionBackupPrivateKey(key, version)
    const restored = await c.getCrypto()!.restoreKeyBackup()
    assert(restored.imported > 0, 'New device must import the backed-up room keys')
    const event = await plaintext(c, roomId, sent.event_id)
    assert.equal(event.getContent().body, message)
    key.fill(0)
    console.log('PASS a fresh MAS device restores encrypted room history from the key backup')
  } finally {
    for (const client of clients) client.stopClient()
  }
}
