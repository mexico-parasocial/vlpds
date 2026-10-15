/* eslint-env node */

// The AppView's data plane as its own process: the API (api.js) reads
// through it, and it is what writes the database. It migrates the database,
// serves the data plane over HTTP, and indexes the PDS firehose
// (RepoSubscription) and bsync's mute and notification operations. Run one:
// the subscriptions are not sharded.
//
// Environment:
//   BSKY_DB_POSTGRES_URL     (required)
//   BSKY_DB_POSTGRES_SCHEMA  optional
//   BSKY_DB_POOL_SIZE        default 10
//   BSKY_DB_MIGRATE          default true; "false" skips migrations
//   BSKY_DATAPLANE_PORT      default 2585
//   BSKY_DID_PLC_URL         default https://plc.directory
//   BSKY_DISABLE_SSRF_PROTECTION  "true" lets identity lookups reach private
//                            addresses, e.g. a PLC in the same compose network
//   BSKY_REPO_PROVIDER       the firehose to index, e.g. ws://pds:2583. Without
//                            it the data plane serves what is already indexed.
//   BSKY_BSYNC_URL, BSKY_BSYNC_API_KEY, BSKY_BSYNC_HTTP_VERSION: optional bsync
//
// The firehose cursor lives in memory and starts at 0, as in dev-env: a
// restart re-indexes what the PDS still holds, which is idempotent.

import assert from 'node:assert'
import {
  BsyncSubscription,
  DataPlaneServer,
  Database,
  RepoSubscription,
} from '@atproto/bsky'

// One JSON line per event, like the services' pino output.
const log = Object.fromEntries(
  ['info', 'warn', 'error'].map((level) => [
    level,
    (fields, msg) => {
      if (typeof fields === 'string') [fields, msg] = [{}, fields]
      console.log(
        JSON.stringify({ level, time: Date.now(), name: 'dataplane', ...fields, msg }),
      )
    },
  ]),
)

const env = {
  dbUrl: process.env.BSKY_DB_POSTGRES_URL,
  dbSchema: process.env.BSKY_DB_POSTGRES_SCHEMA || undefined,
  poolSize: maybeInt(process.env.BSKY_DB_POOL_SIZE) ?? 10,
  migrate: process.env.BSKY_DB_MIGRATE !== 'false',
  port: maybeInt(process.env.BSKY_DATAPLANE_PORT) ?? 2585,
  plcUrl: process.env.BSKY_DID_PLC_URL || 'https://plc.directory',
  disableSsrfProtection: process.env.BSKY_DISABLE_SSRF_PROTECTION === 'true',
  repoProvider: process.env.BSKY_REPO_PROVIDER || undefined,
  bsyncUrl: process.env.BSKY_BSYNC_URL || undefined,
  bsyncApiKey: process.env.BSKY_BSYNC_API_KEY || undefined,
  bsyncHttpVersion: process.env.BSKY_BSYNC_HTTP_VERSION || '2',
}

const main = async () => {
  assert(env.dbUrl, 'must set BSKY_DB_POSTGRES_URL')

  if (env.migrate) {
    const migrationDb = new Database({ url: env.dbUrl, schema: env.dbSchema })
    await migrationDb.migrateToLatestOrThrow()
    await migrationDb.close()
    log.info('dataplane db migrated')
  }

  const db = new Database({
    url: env.dbUrl,
    schema: env.dbSchema,
    poolSize: env.poolSize,
  })
  const dataplane = await DataPlaneServer.create(
    db,
    env.port,
    env.plcUrl,
    // The default fetch refuses private addresses (SSRF protection).
    env.disableSsrfProtection ? globalThis.fetch : undefined,
  )
  log.info({ port: env.port }, 'dataplane is running')

  const repoSub = env.repoProvider
    ? new RepoSubscription({
        service: env.repoProvider,
        db,
        idResolver: dataplane.idResolver,
      })
    : undefined
  if (repoSub) {
    repoSub.start()
    log.info({ service: env.repoProvider }, 'indexing the firehose')
  } else {
    log.warn('BSKY_REPO_PROVIDER is not set: nothing new will be indexed')
  }

  const bsyncSub = env.bsyncUrl
    ? new BsyncSubscription({
        db,
        config: {
          bsyncUrl: env.bsyncUrl,
          bsyncApiKey: env.bsyncApiKey,
          bsyncHttpVersion: env.bsyncHttpVersion,
        },
      })
    : undefined
  bsyncSub?.start()

  process.on('SIGTERM', async () => {
    log.info('dataplane is stopping')
    await bsyncSub?.destroy()
    await repoSub?.destroy()
    await dataplane.destroy()
    await db.close()
    log.info('dataplane is stopped')
  })
}

function maybeInt(str) {
  if (!str) return undefined
  const int = parseInt(str, 10)
  return isNaN(int) ? undefined : int
}

main()
