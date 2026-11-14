// A syncing service's inbound side. As a syncer it's an endpoint registered
// through authorizeSpaceSync's `notify` (http://localhost:<port>, or a fault
// proxy in front of it): it answers confirmRegistration for the tokens it
// issued and takes notifySpaceUpdate, which carries the token and no auth,
// recording each call and handing it to listeners. As a managing app it's a
// real did:plc whose `#atproto_space_syncer` entry points here, answering
// managedspace.checkAccess, whose service auth (iss = the space's
// authority, aud = this service's id, lxm) it verifies against the
// authority's key from the PLC.
import { randomBytes } from 'node:crypto'
import http from 'node:http'
import { verifySignature } from '@atproto/crypto'
import { createServiceDid, signingKey } from './identity.mjs'
import { decodeJwt, spaceDidOf } from './space.mjs'

export const CONFIRM = 'com.atproto.space.confirmRegistration'
export const NOTIFY = 'com.atproto.space.notifySpaceUpdate'
export const CHECK_ACCESS = 'com.atproto.managedspace.checkAccess'
const UNAUTHENTICATED = new Set([CONFIRM, NOTIFY])

export class NotifyService {
  constructor(port, { publicPort } = {}) {
    this.port = port
    this.publicPort = publicPort ?? port
    this.calls = []
    this.listeners = new Set()
    this.authFailures = []
    this.access = () => ({ read: false, write: false }) // checkAccess policy: (space, repo, clientId) => {read, write}, or null for an outage
    this.tokens = new Map() // token -> space
    this.spaceTokens = new Map() // space -> its token (one per space, stable across authorizations)
    this.confirm = () => true // whether to confirm a registration: (space, token) => bool
    this.respond = () => 200 // the status for a notifySpaceUpdate: (call) => status
  }

  get endpoint() {
    return `http://localhost:${this.publicPort}`
  }

  /** The `notify` a syncer of `space` registers with: this endpoint and the space's token. */
  registration(space) {
    let token = this.spaceTokens.get(space)
    if (!token) {
      token = randomBytes(32).toString('base64url')
      this.spaceTokens.set(space, token)
      this.tokens.set(token, space)
    }
    return { endpoint: this.endpoint, token }
  }

  async start() {
    this.server = http.createServer((req, res) => this.handle(req, res))
    await new Promise((r) => this.server.listen(this.port, '127.0.0.1', r))
    const id = await createServiceDid('atproto_space_syncer', `http://localhost:${this.publicPort}`)
    this.did = id.did
    this.serviceRef = id.serviceRef
    return this
  }

  async stop() {
    this.server?.closeAllConnections?.()
    await new Promise((r) => this.server?.close(r) ?? r())
  }

  on(fn) {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }

  callsTo(lxm, space) {
    return this.calls.filter((c) => c.lxm === lxm && (!space || c.body?.space === space))
  }

  async verifyAuth(auth, lxm, space) {
    if (!auth?.startsWith('Bearer ')) return 'no bearer service auth'
    const jwt = auth.slice(7)
    let claims
    try {
      claims = decodeJwt(jwt)
    } catch {
      return 'malformed jwt'
    }
    const { payload } = claims
    const authority = space ? spaceDidOf(space) : undefined
    if (authority && payload.iss !== authority) return `iss ${payload.iss} is not the authority ${authority}`
    if (payload.aud !== this.serviceRef && payload.aud !== this.did) return `aud ${payload.aud} is not ${this.serviceRef}`
    if (payload.lxm !== lxm) return `lxm ${payload.lxm} is not ${lxm}`
    if (!(payload.exp * 1000 > Date.now() - 5000)) return 'expired'
    const [h, p, s] = jwt.split('.')
    try {
      const key = await signingKey(payload.iss)
      const ok = await verifySignature(key, new TextEncoder().encode(`${h}.${p}`), Buffer.from(s, 'base64url'))
      if (!ok) return 'bad signature'
    } catch (e) {
      return `signature check failed: ${e.message}`
    }
    return null
  }

  async handle(req, res) {
    const chunks = []
    for await (const c of req) chunks.push(c)
    const raw = Buffer.concat(chunks).toString('utf8')
    const url = new URL(req.url, 'http://localhost')
    const lxm = url.pathname.replace(/^\/xrpc\//, '')
    let body
    try {
      body = raw ? JSON.parse(raw) : Object.fromEntries(url.searchParams)
    } catch {
      body = raw
    }
    const at = performance.now()
    const authErr = UNAUTHENTICATED.has(lxm)
      ? this.tokens.get(body?.token) === body?.space
        ? null
        : 'a token this service did not issue for the space'
      : await this.verifyAuth(req.headers.authorization, lxm, body?.space)
    const call = { lxm, body, at, wall: Date.now(), authErr }
    this.calls.push(call)
    const reply = (status, obj) => {
      res.writeHead(status, { 'content-type': 'application/json' })
      res.end(JSON.stringify(obj ?? {}))
    }
    if (authErr) {
      this.authFailures.push(call)
      return UNAUTHENTICATED.has(lxm)
        ? reply(400, { error: 'InvalidToken', message: authErr })
        : reply(401, { error: 'AuthRequired', message: authErr })
    }
    if (lxm === CHECK_ACCESS) {
      const a = this.access(body.space, body.repo, body.clientId)
      if (a === null) return reply(503, { error: 'Unavailable', message: 'the managing app is down' })
      return reply(200, { read: !!a.read, write: !!a.write })
    }
    if (lxm === CONFIRM) {
      if (!this.confirm(body.space, body.token)) return reply(400, { error: 'InvalidToken', message: 'not confirmed' })
      return reply(200, { challenge: body.challenge })
    }
    const status = this.respond(call)
    if (status !== 200) return reply(status, { error: status >= 500 ? 'InternalServerError' : 'InvalidRequest' })
    for (const fn of this.listeners) {
      try {
        fn(call)
      } catch (e) {
        console.error('notify listener', e)
      }
    }
    reply(200, {})
  }
}
