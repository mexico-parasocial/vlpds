import { admin } from '../xrpc'
import { buildPut, type Draft, type NodeResult, type ProviderList, type PutOpts } from './signInDraft'
import { K } from './keys'
import { mutate } from './mutate'
import { shared } from './query'

export * from './signInDraft'

// Sign-in providers (docs/operations/sign-in-providers.md): one config object in the bucket, read
// with listSignInProviders and edited with putSignInProvider / removeSignInProvider, each against
// the list's `version` (ifVersion). Secrets are write-only: the list says only whether one is stored.

export const providersQ = shared({
  key: K.signInProviders,
  fn: (signal) => admin<ProviderList>('vlpds.admin.listSignInProviders', { signal }),
  poll: 30_000,
  version: (d) => d.version,
})

export type Saved = { version: number; nodes: NodeResult[] }
const changed = [{ kind: 'config', id: 'signin-providers' }]

export function putProvider(d: Draft, o: PutOpts): Promise<Saved> {
  return mutate({ run: () => admin<Saved>('vlpds.admin.putSignInProvider', { body: buildPut(d, o) }), changes: changed })
}

export function removeProvider(id: string, o: { ifVersion?: number; actor?: string }): Promise<Saved> {
  return mutate({
    run: () => admin<Saved>('vlpds.admin.removeSignInProvider', { body: { id, ifVersion: o.ifVersion, actor: o.actor?.trim() || undefined } }),
    changes: changed,
  })
}
