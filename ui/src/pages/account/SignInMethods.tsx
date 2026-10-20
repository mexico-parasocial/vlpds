import { ErrorNotice, Panel, Status } from '../../components/ui'
import { fmtTime } from '../../lib/format'
import { useLoad } from '../../lib/hooks'
import { acall, XrpcError } from '../../lib/xrpc'

type Identity = { ref: string; issuer: string; provider: string | null; providerName: string | null; display: string | null; addedAt: string; lastUsedAt: string | null; dormant: boolean }
type Provider = { id: string; name: string; icon: string | null }
type Identities = { identities: Identity[]; providers: Provider[]; available: boolean; max: number; manageUrl: string }

const host = (s: string) => {
  try {
    return new URL(s).hostname
  } catch {
    return s
  }
}

/** Identities linked for sign-in with another provider. Adding and removing happen on the server-rendered page `manageUrl`. */
export function SignInMethods() {
  const st = useLoad<Identities>(() => acall('vlpds.server.listFederatedIdentities'), [])
  // a server without sign-in providers has no such method: nothing to show
  if (st.error instanceof XrpcError && (st.error.status === 404 || st.error.status === 501 || st.error.error === 'MethodNotImplemented')) return null
  if (!st.data) {
    if (!st.error) return null
    return (
      <Panel title="Sign-in methods" id="sign-in-methods">
        <ErrorNotice error={st.error} />
      </Panel>
    )
  }
  const d = st.data
  if (!d.available && !d.identities.length) return null
  const icons = new Map(d.providers.map((p) => [p.id, p.icon]))
  return (
    <Panel
      title="Sign-in methods"
      id="sign-in-methods"
      flush={d.identities.length > 0}
      actions={
        <>
          <span className="small muted mono" title="Linked of the most you can link">
            {d.identities.length} of {d.max}
          </span>
          <a className="btn sm" href={d.manageUrl}>
            Manage
          </a>
        </>
      }
    >
      {d.identities.length === 0 ? (
        <span className="muted">None linked</span>
      ) : (
        <div className="table-wrap">
          <table className="data">
            <thead>
              <tr>
                <th>Provider</th>
                <th>Account</th>
                <th className="hide-sm">Added</th>
                <th className="hide-sm">Last used</th>
              </tr>
            </thead>
            <tbody>
              {d.identities.map((x) => {
                const icon = x.provider ? icons.get(x.provider) : null
                return (
                  <tr key={x.ref}>
                    <td>
                      <span className="row" style={{ gap: 8, flexWrap: 'nowrap' }}>
                        {icon && <img src={icon} alt="" width={16} height={16} />}
                        <b className="break">{x.providerName ?? host(x.issuer)}</b>
                        {x.dormant && (
                          <span title="Its provider was removed, so this can’t sign in">
                            <Status kind="warn">Dormant</Status>
                          </span>
                        )}
                      </span>
                    </td>
                    <td className="break">{x.display ?? <span className="muted">—</span>}</td>
                    <td className="nowrap hide-sm">{fmtTime(x.addedAt)}</td>
                    <td className="nowrap hide-sm">{x.lastUsedAt ? fmtTime(x.lastUsedAt) : <span className="muted">Never</span>}</td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      )}
    </Panel>
  )
}
