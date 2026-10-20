import assert from 'node:assert/strict'
import { test } from 'node:test'
import { blankDraft, buildPut, draftOf, hasErrors, validate, type Draft, type ProviderView } from '../src/lib/console/signInDraft.ts'

const google: ProviderView = {
  id: 'google',
  name: 'Google',
  icon: null,
  issuer: 'https://accounts.google.com',
  clientId: '1234.apps.example.com',
  callbackUrl: 'https://pds.example.com/oauth/federated/callback/google',
  clientAuth: { method: 'client_secret_basic', secret: { stored: 'wrapped' }, key: null, keyId: null, signedSecret: null },
  scopes: ['openid', 'email'],
  responseMode: 'query',
  signup: 'off',
  signupsPerDay: 100,
  trustEmail: true,
  requiredClaims: { hd: 'example.com' },
  idTokenAlgs: [],
  updatedAt: '2026-10-19T10:00:00Z',
  updatedBy: 'alex',
}
const apple: ProviderView = {
  ...google,
  id: 'apple',
  name: 'Apple',
  clientAuth: { method: 'client_secret_post', secret: null, key: { stored: 'wrapped' }, keyId: 'ABC123', signedSecret: { issuer: 'TEAMID', lifetimeSecs: 3600 } },
  responseMode: 'form_post',
}
const o = { count: 1, max: 20 }
const pem = (w: string) => `-----${w} ${'PRIVATE'} KEY-----`
const PEM = `${pem('BEGIN')}\nMIGH\n${pem('END')}`

const filled = (): Draft => ({ ...blankDraft(), id: 'ex', name: 'Example', issuer: 'https://id.example.com', clientId: 'abc', secret: { mode: 'value', text: ' s3cret ' } })

test('an edit that leaves the secret on keep omits it', () => {
  const d = draftOf(google)
  assert.equal(d.secret.mode, 'keep')
  assert.equal(hasErrors(validate(d, { ...o, existing: google })), false)
  const b = buildPut(d, { ifVersion: 4 }) as any
  assert.deepEqual(b.provider.clientAuth, { method: 'client_secret_basic' })
  assert.equal(b.ifVersion, 4)
  assert.deepEqual(b.provider.requiredClaims, { hd: 'example.com' })
  assert.equal('scopes' in b.provider, true)
})

test('a typed secret is sent as a value, never a file', () => {
  const d = filled()
  const a = (buildPut(d) as any).provider.clientAuth
  assert.deepEqual(a.secret, { value: 's3cret' })
  assert.equal(JSON.stringify(a).includes('file'), false)
})

test('changing the issuer or the method on an existing provider needs the secret again', () => {
  const d = draftOf(google)
  d.issuer = 'https://other.example.com'
  assert.match(validate(d, { ...o, existing: google }).secret ?? '', /again/)
  d.secret = { mode: 'value', text: 'new' }
  assert.equal(validate(d, { ...o, existing: google }).secret, undefined)
  const m = draftOf(google)
  m.method = 'client_secret_post'
  assert.ok(validate(m, { ...o, existing: google }).secret)
  const same = draftOf(google)
  same.name = 'Google 2'
  assert.equal(validate(same, { ...o, existing: google }).secret, undefined)
})

test('an apple-style provider needs its key again when the issuer changes', () => {
  const d = draftOf(apple)
  d.issuer = 'https://id.example.org'
  assert.ok(validate(d, { ...o, existing: apple }).key)
})

test('ids with surrounding whitespace are refused and trimmed in the body', () => {
  assert.ok(validate({ ...filled(), id: ' ex' }, o).id)
  assert.ok(validate({ ...filled(), id: 'ex ' }, o).id)
  assert.equal((buildPut({ ...filled(), id: 'ex ' }) as any).provider.id, 'ex')
})

test('a new provider needs its secret, an existing one keeps it', () => {
  const d = filled()
  d.secret = { mode: 'value', text: ' ' }
  assert.ok(validate(d, o).secret)
  d.secret = { mode: 'keep', text: '' }
  assert.ok(validate(d, o).secret)
  assert.equal(validate(draftOf(google), { ...o, existing: google }).secret, undefined)
})

test('switching an existing provider to a key does not count its old secret', () => {
  const d = draftOf(google)
  d.method = 'private_key_jwt'
  d.keyId = 'k1'
  assert.ok(validate(d, { ...o, existing: google }).key)
  d.key = { mode: 'value', text: PEM }
  assert.equal(validate(d, { ...o, existing: google }).key, undefined)
  const p = (buildPut(d) as any).provider
  assert.equal('secret' in p.clientAuth, false)
  assert.deepEqual(p.clientAuth.key, { value: PEM })
  assert.equal(p.clientAuth.keyId, 'k1')
})

test('a signed secret needs a key and an issuer, and sends its lifetime as a number', () => {
  const d = draftOf(apple)
  assert.equal(d.key.mode, 'keep')
  assert.equal(hasErrors(validate(d, { ...o, existing: apple })), false)
  const p = (buildPut(d) as any).provider
  assert.deepEqual(p.clientAuth, { method: 'client_secret_post', keyId: 'ABC123', signedSecret: { issuer: 'TEAMID', lifetimeSecs: 3600 } })
  d.signedIssuer = ''
  d.signedLifetime = '1h'
  const e = validate(d, { ...o, existing: apple })
  assert.ok(e.signedIssuer && e.signedLifetime)
})

test('id, issuer, icon, key PEM and claim names are checked', () => {
  const d = { ...filled(), id: 'Bad Id', issuer: 'ftp://x', icon: 'http://x/a.png' }
  const e = validate(d, o)
  assert.ok(e.id && e.issuer && e.icon)
  const k = { ...filled(), method: 'private_key_jwt' as const, keyId: 'k', key: { mode: 'value' as const, text: 'nope' } }
  assert.ok(validate(k, o).key)
  const c = { ...filled(), claims: [{ k: 'hd', v: 'a' }, { k: 'hd', v: 'b' }] }
  assert.ok(validate(c, o).claims)
})

test('an icon over 16 KiB is refused', () => {
  const d = { ...filled(), icon: `data:image/png;base64,${'A'.repeat(17000)}` }
  assert.ok(validate(d, o).icon)
})

test('the 21st provider is refused, an edit of the 20th is not', () => {
  assert.ok(validate(filled(), { count: 20, max: 20 }).max)
  assert.equal(validate(draftOf(google), { count: 20, max: 20, existing: google }).max, undefined)
})

test('force, actor and empty scopes', () => {
  const b = buildPut(filled(), { force: true, actor: ' alex ' }) as any
  assert.equal(b.force, true)
  assert.equal(b.actor, 'alex')
  assert.equal('scopes' in b.provider, false)
  assert.equal('ifVersion' in b, false)
  assert.equal('force' in (buildPut(filled()) as any), false)
})

test('sign-ups per day: blank is omitted, 0 is sent, junk is refused', () => {
  assert.equal('signupsPerDay' in (buildPut(filled()) as any).provider, false)
  assert.equal((buildPut({ ...filled(), signupsPerDay: '0' }) as any).provider.signupsPerDay, 0)
  assert.equal((buildPut({ ...filled(), signupsPerDay: ' 25 ' }) as any).provider.signupsPerDay, 25)
  assert.ok(validate({ ...filled(), signupsPerDay: '-1' }, o).signupsPerDay)
  assert.ok(validate({ ...filled(), signupsPerDay: '1.5' }, o).signupsPerDay)
  assert.equal(draftOf(google).signupsPerDay, '100')
})
