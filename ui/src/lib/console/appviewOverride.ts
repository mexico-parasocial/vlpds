import { admin } from '../xrpc'
import { K } from './keys'
import { mutate } from './mutate'
import { shared } from './query'

// The AppView override (docs/proxying.md "AppView override"): getAppviewOverride read as a
// series, its traffic counters turned into rates between polls, and the edit model. Every
// change is a small edit of the stored config, shown as a diff before updateAppviewOverride
// saves it with ifVersion.

export type OvDoc = {
  version?: number
  enabled?: boolean
  url: string
  did: string
  accounts: string[]
  methods: string[]
  fallback: boolean
  updatedAt?: string
  updatedBy?: string
  note?: string
  history?: OvAudit[]
}
export type OvAudit = { version: number; at: string; by: string; ip?: string; node: string; note?: string; changes: string[] }
export type OvError = { version: number | null; message: string; atMs: number }
/** Counters since each node started, summed. A fallback's 501 or 400 is also in serverError or clientError. */
export type OvStats = {
  ok: number
  clientError: number
  serverError: number
  unreachable: number
  fallbacks: number
  fallbackServerErrors: number
  fallbackClientErrors: number
  headSecondsSum: number
  headCount: number
}
export type OvNode = {
  node: string
  self: boolean
  reachable: boolean
  unsupported?: boolean
  source?: 'flags' | 'object'
  configVersion?: number
  configError?: OvError | null
  loadedAtMs?: number | null
  checkedAtMs?: number | null
  active?: boolean
  accounts?: number
  appviewConfigured?: boolean
  stats?: OvStats
}
export type AppviewOverride = {
  node: string
  source: 'flags' | 'object'
  config: OvDoc | null
  configVersion: number
  configError: OvError | null
  flags: OvDoc | null
  effective: OvDoc | null
  defaultAppview: { url: string; did: string } | null
  refreshSecs: number
  limits: { accounts: number; methods: number }
  stats: OvStats
  nodes: OvNode[]
  unreachableNodes?: string[]
  time: number
}

/** The AppView methods offered as checkboxes: what the Bluesky app reads most, by area. */
export const METHOD_GROUPS: [string, string[]][] = [
  ['Feeds and threads', ['getTimeline', 'getAuthorFeed', 'getPostThread', 'getPosts', 'getFeed', 'getListFeed', 'getActorLikes', 'getLikes', 'getRepostedBy', 'getQuotes'].map((m) => `app.bsky.feed.${m}`)],
  ['Feed generators', ['getFeedGenerator', 'getFeedGenerators', 'getActorFeeds'].map((m) => `app.bsky.feed.${m}`)],
  ['Profiles', ['getProfile', 'getProfiles'].map((m) => `app.bsky.actor.${m}`)],
  [
    'Graph',
    ['getFollows', 'getFollowers', 'getKnownFollowers', 'getRelationships', 'getBlocks', 'getMutes', 'getList', 'getLists', 'getListBlocks', 'getListMutes', 'getStarterPack', 'getStarterPacks', 'getActorStarterPacks'].map(
      (m) => `app.bsky.graph.${m}`,
    ),
  ],
  ['Notifications', ['listNotifications', 'getUnreadCount'].map((m) => `app.bsky.notification.${m}`)],
  ['Labelers', ['app.bsky.labeler.getServices']],
]
export const LISTED_METHODS = new Set(METHOD_GROUPS.flatMap(([, ms]) => ms))
/** Left off the checklist: they stay on the default AppView unless typed in. */
export const LEFT_OUT = 'suggestions, search, unspecced and trending'

export const shortMethod = (m: string) => m.replace(/^app\.bsky\./, '')

// ---------------------------------------------------------------- the read, and rates between polls

const POLL = 5000
const KEEP = 72 // 6 minutes at 5 s
type Sample = { t: number; s: OvStats }
let samples: Sample[] = []

export type Loaded = AppviewOverride & { fetchedAt: number }

export const overrideQ = shared({
  key: K.appviewOverride,
  fn: async (signal): Promise<Loaded> => {
    const d = await admin<AppviewOverride>('vlpds.admin.getAppviewOverride', { signal })
    const t = Date.now()
    samples = [...samples, { t: t / 1000, s: d.stats }].slice(-(KEEP + 1))
    rates = undefined
    return { ...d, fetchedAt: t }
  },
  poll: POLL,
  stream: true,
  version: (d) => d.time,
})

/** The config alone, from the node you reach (no fan-out, no series): for an account's page. */
export const overrideCfgQ = shared({
  key: K.appviewOverrideCfg,
  fn: (signal): Promise<AppviewOverride> => admin<AppviewOverride>('vlpds.admin.getAppviewOverride', { params: { local: true }, signal }),
  poll: 30_000,
  version: (d) => d.time,
})

export type Rates = { requests: number[]; fallbacks: number[]; errors: number[]; clientErrors: number[]; headMs: (number | null)[]; pollSecs: number }
let rates: Rates | undefined

const total = (s: OvStats) => s.ok + s.clientError + s.serverError + s.unreachable
/** 5xx and unreachable, not counting the 501s that fell back. */
const errors = (s: OvStats) => s.serverError - (s.fallbackServerErrors ?? 0) + s.unreachable
const clientErrors = (s: OvStats) => s.clientError - (s.fallbackClientErrors ?? 0)

/** Per second since the console started polling, from the change in each cluster total between polls. */
export function overrideRates(): Rates {
  if (rates) return rates
  const r: Rates = { requests: [], fallbacks: [], errors: [], clientErrors: [], headMs: [], pollSecs: POLL / 1000 }
  for (let i = 1; i < samples.length; i++) {
    const a = samples[i - 1]
    const b = samples[i]
    const dt = b.t - a.t
    // a node restarting resets its counters: never a negative rate
    const per = (f: (s: OvStats) => number) => (dt > 0 ? Math.max(0, f(b.s) - f(a.s)) / dt : 0)
    r.requests.push(per(total))
    r.fallbacks.push(per((s) => s.fallbacks))
    r.errors.push(per(errors))
    r.clientErrors.push(per(clientErrors))
    const n = b.s.headCount - a.s.headCount
    r.headMs.push(n > 0 ? ((b.s.headSecondsSum - a.s.headSecondsSum) / n) * 1000 : null)
  }
  rates = r
  return r
}

export const lifetime = (s: OvStats) => ({ requests: total(s), errors: errors(s), clientErrors: clientErrors(s), meanHeadMs: s.headCount ? (s.headSecondsSum / s.headCount) * 1000 : undefined })

// ---------------------------------------------------------------- the edit model

export type Draft = { enabled: boolean; url: string; did: string; accounts: string[]; methods: string[]; fallback: boolean }

/** What a save starts from: the stored object, else the flags this node booted with, else nothing. */
export function draftOf(d: AppviewOverride): Draft {
  const doc = d.config ?? d.flags
  return {
    enabled: doc?.enabled ?? true,
    url: doc?.url ?? '',
    did: doc?.did ?? '',
    accounts: [...(doc?.accounts ?? [])],
    methods: [...(doc?.methods ?? [])],
    fallback: doc?.fallback ?? true,
  }
}

/** The stored version an edit is made against (an object a node rejected still counts). */
export function baseVersion(d: AppviewOverride): number {
  let v = d.config?.version ?? 0
  for (const n of d.nodes) if (n.configError?.version != null) v = Math.max(v, n.configError.version)
  return v
}

/** One line per change: [what, before, after]; before or after empty for added or removed. */
export function diffDrafts(a: Draft, b: Draft, label: (did: string) => string = (x) => x): [string, string, string][] {
  const out: [string, string, string][] = []
  const onoff = (v: boolean) => (v ? 'on' : 'off')
  if (a.enabled !== b.enabled) out.push(['Override', onoff(a.enabled), onoff(b.enabled)])
  if (a.url !== b.url) out.push(['URL', a.url || '—', b.url || '—'])
  if (a.did !== b.did) out.push(['Service DID', a.did || '—', b.did || '—'])
  if (a.fallback !== b.fallback) out.push(['Fallback', onoff(a.fallback), onoff(b.fallback)])
  for (const m of b.methods) if (!a.methods.includes(m)) out.push(['Method', '', shortMethod(m)])
  for (const m of a.methods) if (!b.methods.includes(m)) out.push(['Method', shortMethod(m), ''])
  for (const x of b.accounts) if (!a.accounts.includes(x)) out.push(['Account', '', label(x)])
  for (const x of a.accounts) if (!b.accounts.includes(x)) out.push(['Account', label(x), ''])
  return out
}

export type SaveResult = { version: number; nodes: { node: string; ok: boolean; configVersion?: number; error?: string }[] }

export function saveDraft(d: AppviewOverride, draft: Draft, actor: string, note: string): Promise<SaveResult> {
  return mutate({
    run: () =>
      admin<SaveResult>('vlpds.admin.updateAppviewOverride', {
        body: { config: draft, ifVersion: baseVersion(d), actor: actor.trim() || undefined, note: note.trim() || undefined },
      }),
    changes: [{ kind: 'config', id: 'appview-override' }],
  })
}

/** The current config, fresh: edits start from what the server has now. */
export const freshOverride = (): Promise<Loaded> => overrideQ.fresh()

export const validUrl = (u: string) => {
  try {
    const x = new URL(u)
    return (x.protocol === 'http:' || x.protocol === 'https:') && !!x.hostname
  } catch {
    return false
  }
}
export const validDid = (d: string) => /^did:[a-z]+:[a-zA-Z0-9._:%-]+$/.test(d)
export const validMethod = (m: string) => /^app\.bsky(\.[a-zA-Z0-9-]+)+\.[a-zA-Z][a-zA-Z0-9]*$/.test(m)
