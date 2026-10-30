#!/usr/bin/env node
// Prepare a private, reviewable proposal; application never regenerates MAS keys.
import { createHash } from 'node:crypto'
import { createRequire } from 'node:module'
import { chmod, chown, copyFile, readFile, rename, stat, writeFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

const require = createRequire(new URL('../../ui/package.json', import.meta.url))
const yaml = require('js-yaml')
export const CLIENT_ID = '01K3ZQD8N7P0J5R2T9V4W6X8YD'
const hash = value => createHash('sha256').update(value).digest('hex')

export function upgrade(source, secret) {
  if (!/^[a-f0-9]{64}$/.test(secret)) throw Error('Expected a 64-character lowercase hex secret')
  if (Buffer.byteLength(source) > 1024 * 1024) throw Error('MAS configuration exceeds limit')
  let config
  try { config = yaml.load(source, { schema: yaml.CORE_SCHEMA }) }
  catch { throw Error('Cannot parse MAS configuration; source preserved') }
  if (!config?.secrets?.encryption || !Array.isArray(config.secrets.keys) || !Array.isArray(config.clients) ||
      !Array.isArray(config.http?.listeners) || !config.matrix || !config.upstream_oauth2?.providers) {
    throw Error('Unrecognized MAS configuration; manual review required')
  }
  const protectedHash = hash(JSON.stringify([config.secrets, config.matrix, config.database, config.upstream_oauth2, config.clients.filter(c => c.client_id !== CLIENT_ID)]))
  const internal = config.http.listeners.filter(l => l.name === 'internal')
  if (internal.length !== 1 || !Array.isArray(internal[0].resources) || !Array.isArray(internal[0].binds)) throw Error('Expected one existing internal listener')
  // Never add a sensitive resource to a public/shared listener.
  if (internal[0].resources.some(r => !['health', 'adminapi', 'metrics'].includes(r.name)) ||
      internal[0].binds.some(b => b.port !== 8081)) throw Error('Internal listener requires manual review')
  const existing = config.clients.filter(c => c.client_id === CLIENT_ID)
  if (existing.length > 1 || (existing[0] && (existing[0].client_auth_method !== 'client_secret_basic' || existing[0].client_secret !== secret))) {
    throw Error('Bridge client collision; no credential was changed')
  }
  config.policy ??= {}
  config.policy.data ??= {}
  config.policy.data.admin_clients ??= []
  if (!Array.isArray(config.policy.data.admin_clients) || config.policy.data.admin_clients.some(c => typeof c !== 'string')) throw Error('Invalid admin client policy')
  if (!internal[0].resources.some(r => r.name === 'adminapi')) internal[0].resources.push({ name: 'adminapi' })
  if (!existing.length) config.clients.push({ client_id: CLIENT_ID, client_auth_method: 'client_secret_basic', client_secret: secret })
  if (!config.policy.data.admin_clients.includes(CLIENT_ID)) config.policy.data.admin_clients.push(CLIENT_ID)
  return { text: yaml.dump(config, { schema: yaml.CORE_SCHEMA, lineWidth: -1, noRefs: true }), protectedHash }
}

async function privateRead(path) {
  const metadata = await stat(path)
  if (!metadata.isFile() || metadata.size > 1024 * 1024) throw Error('Invalid or oversized private input')
  return readFile(path, 'utf8')
}

export async function main(args) {
  const [action, input, secretPath] = args
  if (!['prepare', 'apply'].includes(action) || !input || (action === 'prepare' && !secretPath)) {
    throw Error('Usage: upgrade-mas-admin.mjs prepare CONFIG SECRET_FILE | apply PROPOSAL_JSON')
  }
  process.umask(0o077)
  if (action === 'prepare') {
    const sourcePath = resolve(input)
    const source = await privateRead(sourcePath)
    const secret = (await privateRead(resolve(secretPath))).trim()
    const proposed = upgrade(source, secret)
    const proposalPath = `${sourcePath}.bridge-admin.proposed.yaml`
    const metadataPath = `${sourcePath}.bridge-admin.proposal.json`
    await writeFile(proposalPath, proposed.text, { mode: 0o600, flag: 'wx' })
    await writeFile(metadataPath, JSON.stringify({ version: 1, sourcePath, proposalPath, sourceHash: hash(source),
      proposalHash: hash(proposed.text), protectedHash: proposed.protectedHash, secretHash: hash(secret) }, null, 2), { mode: 0o600, flag: 'wx' })
    console.log(`Prepared private proposal: ${metadataPath}\nReview locally: adds only the private admin resource, bridge client and policy. Preserve the original keys.`)
    return
  }
  const metadata = JSON.parse(await privateRead(resolve(input)))
  if (metadata.version !== 1 || !metadata.sourcePath || !metadata.proposalPath) throw Error('Invalid proposal metadata')
  const source = await privateRead(metadata.sourcePath)
  const proposal = await privateRead(metadata.proposalPath)
  if (hash(source) !== metadata.sourceHash || hash(proposal) !== metadata.proposalHash) throw Error('Source or proposal changed; prepare a new review')
  let secret
  try { secret = yaml.load(proposal, { schema: yaml.CORE_SCHEMA }).clients.find(c => c.client_id === CLIENT_ID).client_secret }
  catch { throw Error('Invalid private proposal') }
  const expected = upgrade(source, secret)
  if (expected.text !== proposal || expected.protectedHash !== metadata.protectedHash || hash(secret) !== metadata.secretHash) throw Error('Proposal failed preservation checks')
  const original = await stat(metadata.sourcePath)
  const backup = `${metadata.sourcePath}.before-bridge-admin-${Date.now()}`
  await copyFile(metadata.sourcePath, backup, 1)
  await chmod(backup, 0o600)
  const temporary = `${metadata.sourcePath}.bridge-admin.tmp`
  await writeFile(temporary, proposal, { mode: 0o600, flag: 'wx' })
  await chown(temporary, original.uid, original.gid)
  await chmod(temporary, original.mode & 0o777)
  await rename(temporary, metadata.sourcePath)
  console.log('Applied the reviewed configuration; original private backup preserved. Restart MAS and bridge in a maintenance window.')
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main(process.argv.slice(2)).catch(error => { console.error(error.message); process.exitCode = 1 })
}
