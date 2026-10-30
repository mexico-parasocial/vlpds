import assert from 'node:assert/strict'
import { test } from 'node:test'
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { upgrade, main } from './upgrade-mas-admin.mjs'

const fixture = `http:
  listeners:
    - name: internal
      resources: [{name: health}]
      binds: [{host: '0.0.0.0', port: 8081}]
secrets: {encryption: old-encryption, keys: [{kid: stable, key: private-rsa}]}
matrix: {secret: stable-synapse}
database: {uri: private-database}
upstream_oauth2: {providers: [{id: stable-provider}]}
clients: [{client_id: old-client, client_secret: old-secret}]
`
const secret = 'a'.repeat(64)

test('preserves all existing cryptographic/account-link configuration and is idempotent', () => {
  const first = upgrade(fixture, secret)
  const second = upgrade(first.text, secret)
  assert.equal(first.protectedHash, second.protectedHash)
  assert.equal(first.text, second.text)
  assert.match(first.text, /old-encryption/)
  assert.match(first.text, /stable-provider/)
  assert.match(first.text, /old-secret/)
  assert.throws(() => upgrade(first.text, 'b'.repeat(64)), /collision/)
})
test('rejects duplicate YAML fields and a shared public listener', () => {
  assert.throws(() => upgrade(fixture + '\nsecrets: {}\n', secret), /Cannot parse/)
  assert.throws(() => upgrade(fixture.replace('name: health', 'name: oauth'), secret), /manual review/)
})
test('refuses to overwrite a configuration that changed after proposal preparation', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'para-mas-upgrade-'))
  try {
    const config = join(dir, 'config.yaml'), secretFile = join(dir, 'secret')
    await writeFile(config, fixture); await writeFile(secretFile, secret)
    await main(['prepare', config, secretFile])
    await writeFile(config, fixture + '# changed\n')
    await assert.rejects(main(['apply', config + '.bridge-admin.proposal.json']), /changed/)
    assert.equal(await readFile(config, 'utf8'), fixture + '# changed\n')
  } finally { await rm(dir, { recursive: true, force: true }) }
})
test('applies only the reviewed bytes and preserves a private original backup', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'para-mas-upgrade-'))
  try {
    const config = join(dir, 'config.yaml'), secretFile = join(dir, 'secret')
    await writeFile(config, fixture, { mode: 0o600 }); await writeFile(secretFile, secret, { mode: 0o600 })
    await main(['prepare', config, secretFile])
    await main(['apply', config + '.bridge-admin.proposal.json'])
    assert.equal(await readFile(config, 'utf8'), upgrade(fixture, secret).text)
  } finally { await rm(dir, { recursive: true, force: true }) }
})
