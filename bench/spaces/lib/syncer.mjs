// A space syncer built on the reference library: the boards app's
// SpaceSyncer (packages/boards/src/spaces/syncer.mjs), with credentials from a
// harness member account's delegation and KEM keys from the local PLC.
import { SpaceSyncer, hashEq } from '../../../../boards/src/spaces/syncer.mjs'
import { attempt } from './http.mjs'
import { credentialFor } from './space.mjs'

export { hashEq }

export class Syncer extends SpaceSyncer {
  /**
   * `member`: the account whose delegation the syncer's credentials come
   * from (a reader). `notify`: the `{endpoint, token}` it registers with
   * each authorization (a NotifyService's `registration(space)`).
   */
  constructor(name, space, member, { snapshot, notify } = {}) {
    super(name, space, { mint: (opts) => credentialFor(member, space, opts), attempt, snapshot, notify })
    this.member = member
  }
}
