// Run from the sibling iM8 checkout with pnpm exec tsx <absolute script path>.
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { pdsAuthPublicKey, signPdsChallenge } from '../../../iM8/src/services/pdsAuth.ts'

const exec = promisify(execFile)
const name = `para-wallet-smoke-${process.pid}-${Date.now()}`
const image = process.env.PARA_NATIVE_IMAGE ?? 'para-vlpds:para-native-local'
const password = 'synthetic-wallet-test-password'
const admin = `Basic ${Buffer.from('admin:synthetic-admin-test-token').toString('base64')}`
const seed = new Uint8Array(32).fill(42)

try {
  await exec('docker', ['run', '-d', '--name', name, '-p', '127.0.0.1::2583', image,
    '--memory', '--dev-mode', '--para', '--plc-mode', 'unregistered', '--no-rate-limits',
    '--listen', '0.0.0.0:2583', '--public-url', 'http://127.0.0.1:2583',
    '--service-did', 'did:web:wallet.smoke.test', '--admin-token', 'synthetic-admin-test-token',
    '--io-threads', '2', '--workers', '2', '--shards', '8', '--crawlers='])
  const { stdout } = await exec('docker', ['port', name, '2583/tcp'])
  const base = `http://${stdout.trim()}`
  let ready = false
  for (let n = 0; n < 100; n++) {
    try { ready = (await fetch(`${base}/xrpc/_health`, { signal: AbortSignal.timeout(1000) })).ok } catch {}
    if (ready) break
    await new Promise(resolve => setTimeout(resolve, 250))
  }
  if (!ready) throw new Error('Native Linux PDS did not become healthy')
  const request = async (method: string, body?: unknown, auth?: string, expected = 200) => {
    const r = await fetch(`${base}/xrpc/${method}`, {
      method: body === undefined ? 'GET' : 'POST',
      headers: { ...(auth ? { Authorization: auth } : {}), ...(body === undefined ? {} : { 'content-type': 'application/json' }) },
      body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(10000),
    })
    const value = await r.json()
    if (r.status !== expected) throw new Error(`${method}: ${r.status} ${value.error ?? ''}, expected ${expected}`)
    return value
  }
  const account = await request('com.atproto.server.createAccount', { handle: 'walletsmoke.vlpds.test', email: 'walletsmoke@example.com', password })
  const binding = { did: account.did as string, audience: 'did:web:wallet.smoke.test' }
  const enrollment = await request('vlpds.para.startIm8Enrollment', { publicKey: pdsAuthPublicKey(seed, binding) }, `Bearer ${account.accessJwt}`)
  await request('com.para.account.setAuthFactor', { authFactorType: 'im8', password,
    authFactorToken: signPdsChallenge(seed, enrollment.challenge, { ...binding, purpose: 'enroll' }) }, `Bearer ${account.accessJwt}`)
  await request('com.atproto.server.createSession', { identifier: account.handle, password }, undefined, 401)
  const challenge = await request(`vlpds.para.getIm8Challenge?identifier=${encodeURIComponent(account.handle)}`)
  const token = signPdsChallenge(seed, challenge.challenge, { ...binding, purpose: 'login' })
  const session = await request('com.atproto.server.createSession', { identifier: account.handle, password, authFactorToken: token })
  await request('com.atproto.server.createSession', { identifier: account.handle, password, authFactorToken: token }, undefined, 401)
  const auth = `Bearer ${session.accessJwt}`
  await request('com.para.alpha.createInvite', { state: 'NL' }, auth, 401)
  await request('vlpds.para.setAlphaRollout', { state: 'NL', totalSlots: 1, isOpen: true }, admin)
  const approved = await request('com.para.alpha.requestAccess', { state: 'NL' }, auth)
  if (approved.status !== 'approved') throw new Error('Alpha allocation failed')
  const access = await request('com.para.alpha.getAccess', undefined, auth)
  if (!access.hasAccess) throw new Error('Alpha approval was not stored')
  console.log('PASS: JavaScript wallet → Linux PDS enrollment/login, replay rejection and private alpha authorization')
} catch (error) {
  const logs=await exec('docker',['logs',name]).catch(()=>({stdout:'',stderr:''}))
  process.stderr.write(logs.stdout+logs.stderr)
  throw error
} finally {
  seed.fill(0)
  await exec('docker', ['rm', '-f', name]).catch(() => undefined)
}
