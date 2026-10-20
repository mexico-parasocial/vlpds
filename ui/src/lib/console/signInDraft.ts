// The sign-in provider form's model: the shapes vlpds.admin.listSignInProviders answers with, the
// draft an operator edits, its checks and the putSignInProvider body it builds. No imports, so
// test/signInDraft.test.ts runs it under node as it is.

export type Stored = null | { stored: 'wrapped' }
export type ClientMethod = 'client_secret_basic' | 'client_secret_post' | 'private_key_jwt'
export type ResponseMode = 'query' | 'form_post'
export type Signup = 'off' | 'invite' | 'open'
export type SignedSecret = { issuer: string; audience?: string; lifetimeSecs?: number }

export type ProviderView = {
  id: string
  name: string
  icon: string | null
  issuer: string
  clientId: string
  /** {public_url}/oauth/federated/callback/{id}: what to register at the provider. */
  callbackUrl: string
  clientAuth: { method: ClientMethod; secret: Stored; key: Stored; keyId: string | null; signedSecret: SignedSecret | null }
  scopes: string[]
  responseMode: ResponseMode
  signup: Signup
  /** Accounts created through it per UTC day, cluster-wide; 0 is no cap. */
  signupsPerDay: number
  trustEmail: boolean
  requiredClaims: Record<string, string>
  idTokenAlgs: string[]
  updatedAt: string
  updatedBy: string
}

export type ProviderList = {
  providers: ProviderView[]
  version: number
  updatedAt: string | null
  max: number
  featureLevel: { active: number | null; required: number }
  available: boolean
  reason?: string
}

export type NodeResult = { node: string; ok: boolean; error?: string }

export const ID_ALGS = ['RS256', 'RS384', 'RS512', 'PS256', 'PS384', 'PS512', 'ES256', 'ES384', 'EdDSA']
export const DEFAULT_ALGS = ['RS256', 'PS256', 'ES256', 'EdDSA']
export const ICON_MAX = 16 * 1024
export const METHODS: ClientMethod[] = ['client_secret_basic', 'client_secret_post', 'private_key_jwt']

/** A write-only credential: keep what is stored, or type a new value. */
export type Cred = { mode: 'keep' | 'value'; text: string }

export type Draft = {
  id: string
  name: string
  icon: string | null
  issuer: string
  clientId: string
  method: ClientMethod
  secret: Cred
  key: Cred
  keyId: string
  signed: boolean
  signedIssuer: string
  signedAudience: string
  signedLifetime: string
  scopes: string
  responseMode: ResponseMode
  signup: Signup
  signupsPerDay: string
  trustEmail: boolean
  claims: { k: string; v: string }[]
  algs: string[]
}

export const blankDraft = (): Draft => ({
  id: '',
  name: '',
  icon: null,
  issuer: '',
  clientId: '',
  method: 'client_secret_basic',
  secret: { mode: 'value', text: '' },
  key: { mode: 'value', text: '' },
  keyId: '',
  signed: false,
  signedIssuer: '',
  signedAudience: '',
  signedLifetime: '',
  scopes: '',
  responseMode: 'query',
  signup: 'off',
  signupsPerDay: '',
  trustEmail: false,
  claims: [],
  algs: [],
})

const credOf = (s: Stored): Cred => (s ? { mode: 'keep', text: '' } : { mode: 'value', text: '' })

export function draftOf(p: ProviderView): Draft {
  const a = p.clientAuth
  return {
    id: p.id,
    name: p.name,
    icon: p.icon,
    issuer: p.issuer,
    clientId: p.clientId,
    method: a.method,
    secret: credOf(a.secret),
    key: credOf(a.key),
    keyId: a.keyId ?? '',
    signed: !!a.signedSecret,
    signedIssuer: a.signedSecret?.issuer ?? '',
    signedAudience: a.signedSecret?.audience ?? '',
    signedLifetime: a.signedSecret?.lifetimeSecs != null ? String(a.signedSecret.lifetimeSecs) : '',
    scopes: p.scopes.join(' '),
    responseMode: p.responseMode,
    signup: p.signup,
    signupsPerDay: String(p.signupsPerDay),
    trustEmail: p.trustEmail,
    claims: Object.entries(p.requiredClaims).map(([k, v]) => ({ k, v })),
    algs: [...p.idTokenAlgs],
  }
}

/** Which credentials the chosen client auth uses: a secret, or a key (private_key_jwt, or a secret the PDS signs). */
export const usesSecret = (d: Pick<Draft, 'method' | 'signed'>) => d.method !== 'private_key_jwt' && !d.signed
export const usesKey = (d: Pick<Draft, 'method' | 'signed'>) => d.method === 'private_key_jwt' || d.signed

export const splitScopes = (s: string) => s.split(/[\s,]+/).filter(Boolean)

const ID_RE = /^[a-z0-9-]{1,32}$/
export const validId = (s: string) => ID_RE.test(s)
export const validIssuer = (s: string) => {
  try {
    const u = new URL(s)
    return (u.protocol === 'https:' || u.protocol === 'http:') && !!u.hostname && !u.search && !u.hash
  } catch {
    return false
  }
}
/** Bytes of a data: URI as the server counts them (the whole string). */
export const iconBytes = (s: string) => new TextEncoder().encode(s).length

export type Errors = Partial<Record<'id' | 'name' | 'icon' | 'issuer' | 'clientId' | 'secret' | 'key' | 'keyId' | 'signedIssuer' | 'signedLifetime' | 'signupsPerDay' | 'claims' | 'max', string>>

const PEM_BEGIN = new RegExp(`-----BEGIN (?:EC )?${'PRIVATE'} KEY-----`)

const credError = (c: Cred, has: boolean, what: 'secret' | 'key', again: boolean): string | undefined => {
  if (c.mode === 'keep') return has && !again ? undefined : again && has ? 'Enter it again: the issuer or auth method changed' : `A ${what} is required`
  const t = c.text.trim()
  if (!t) return `A ${what} is required`
  if (what === 'key' && !PEM_BEGIN.test(t)) return 'A P-256 PKCS#8 PEM'
  return undefined
}

/**
 * What stops a save. `existing` is the stored provider being edited (its stored secret and key
 * count as present), `count` how many providers there are now.
 */
export function validate(d: Draft, o: { existing?: ProviderView; count: number; max: number }): Errors {
  const e: Errors = {}
  if (!o.existing && o.count >= o.max) e.max = `At most ${o.max} providers`
  if (!d.id) e.id = 'Required'
  else if (!validId(d.id)) e.id = 'a–z, 0–9 and -, up to 32'
  if (!d.name.trim()) e.name = 'Required'
  if (!d.issuer.trim()) e.issuer = 'Required'
  else if (!validIssuer(d.issuer.trim())) e.issuer = 'An https:// URL without a query or fragment'
  if (!d.clientId.trim()) e.clientId = 'Required'
  if (d.icon !== null) {
    if (!d.icon.startsWith('data:image/')) e.icon = 'A data:image/ URI'
    else if (iconBytes(d.icon) > ICON_MAX) e.icon = `${(iconBytes(d.icon) / 1024).toFixed(1)} KiB of 16`
  }
  // a stored credential only carries over while the issuer and auth method are the ones it was saved for
  const again = !!o.existing && (d.issuer.trim() !== o.existing.issuer || d.method !== o.existing.clientAuth.method)
  if (usesSecret(d)) {
    const s = credError(d.secret, !!o.existing?.clientAuth.secret && o.existing.clientAuth.method !== 'private_key_jwt', 'secret', again)
    if (s) e.secret = s
  }
  if (usesKey(d)) {
    const k = credError(d.key, !!o.existing?.clientAuth.key, 'key', again)
    if (k) e.key = k
    if (!d.keyId.trim()) e.keyId = 'Required'
  }
  if (d.signed) {
    if (!d.signedIssuer.trim()) e.signedIssuer = 'Required'
    const l = d.signedLifetime.trim()
    if (l && !(/^\d+$/.test(l) && Number(l) > 0)) e.signedLifetime = 'Whole seconds'
  }
  const n = d.signupsPerDay.trim()
  if (n && !/^\d+$/.test(n)) e.signupsPerDay = 'Whole accounts, 0 for no cap'
  const keys = d.claims.map((c) => c.k.trim())
  if (keys.some((k) => !k)) e.claims = 'A claim needs a name'
  else if (new Set(keys).size !== keys.length) e.claims = 'Claim names are unique'
  return e
}

export const hasErrors = (e: Errors) => Object.keys(e).length > 0

const credBody = (c: Cred): { value: string } | undefined => (c.mode === 'keep' ? undefined : { value: c.text.trim() })

export type PutOpts = { ifVersion?: number; force?: boolean; actor?: string }

/** The putSignInProvider body. A credential left on "keep" is left out, so the stored one stays. */
export function buildPut(d: Draft, o: PutOpts = {}) {
  const auth: Record<string, unknown> = { method: d.method }
  if (usesSecret(d)) {
    const s = credBody(d.secret)
    if (s) auth.secret = s
  }
  if (usesKey(d)) {
    const k = credBody(d.key)
    if (k) auth.key = k
    auth.keyId = d.keyId.trim()
  }
  if (d.signed) {
    const ss: SignedSecret = { issuer: d.signedIssuer.trim() }
    if (d.signedAudience.trim()) ss.audience = d.signedAudience.trim()
    if (d.signedLifetime.trim()) ss.lifetimeSecs = Number(d.signedLifetime.trim())
    auth.signedSecret = ss
  }
  const scopes = splitScopes(d.scopes)
  const provider: Record<string, unknown> = {
    id: d.id.trim(),
    name: d.name.trim(),
    issuer: d.issuer.trim(),
    clientId: d.clientId.trim(),
    clientAuth: auth,
    responseMode: d.responseMode,
    signup: d.signup,
    trustEmail: d.trustEmail,
    requiredClaims: Object.fromEntries(d.claims.map((c) => [c.k.trim(), c.v])),
    idTokenAlgs: d.algs,
  }
  if (d.signupsPerDay.trim()) provider.signupsPerDay = Number(d.signupsPerDay.trim())
  if (d.icon) provider.icon = d.icon
  if (scopes.length) provider.scopes = scopes
  const body: Record<string, unknown> = { provider }
  if (o.ifVersion !== undefined) body.ifVersion = o.ifVersion
  if (o.force) body.force = true
  if (o.actor?.trim()) body.actor = o.actor.trim()
  return body
}

/** A credential's state as a word for the list and form. */
export const storedText = (s: Stored) => (s ? 'stored' : 'none')

/** The credential the chosen method actually uses, for the list's "client auth" cell. */
export const authLabel = (m: ClientMethod) => m.replace(/^client_secret_/, 'secret ').replace('private_key_jwt', 'key jwt')
