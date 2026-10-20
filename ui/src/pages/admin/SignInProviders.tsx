import { DataTable, type Col } from '../../components/console/DataTable'
import { Banners, Chip, Empty, ErrorState, Glyph, Loading, NeedsVersion, PageHead, Panel, Src, type BannerSpec } from '../../components/console/kit'
import '../../console-signin.css'
import { ago } from '../../lib/console/fmt'
import { providersQ, type ProviderList, type ProviderView } from '../../lib/console/signInProviders'
import { missing } from '../../lib/console/sys'
import { Link } from '../../lib/router'
import { addProvider, ProviderIcon } from './signInUi'

// Sign-in providers (docs/operations/sign-in-providers.md): the OIDC providers people can sign in
// with, one config object saved with ifVersion.

export function SignInProviders() {
  const s = providersQ.use()
  const d = s.data
  if (!d) {
    if (s.error && missing(s.error))
      return (
        <>
          <PageHead title="Sign-in providers" />
          <Panel>
            <NeedsVersion what="Sign-in providers" nsid="vlpds.admin.listSignInProviders" />
          </Panel>
        </>
      )
    return s.error ? <ErrorState error={s.error} retry={providersQ.refresh} /> : <Loading />
  }
  return <Page d={d} at={s.at} stale={!!s.error} />
}

function Page({ d, at, stale }: { d: ProviderList; at?: number; stale: boolean }) {
  const banners: BannerSpec[] = []
  if (stale) banners.push({ id: 'stale', tone: 'warn', title: 'Not updating', desc: 'listSignInProviders is failing; showing the last answer.' })
  return (
    <>
      <PageHead
        title="Sign-in providers"
        sub={
          <>
            <span>
              {d.providers.length} of {d.max}
            </span>
            <span>
              feature level {d.featureLevel.active ?? '—'} <span className="muted">(needs {d.featureLevel.required})</span>
            </span>
            {d.available && d.version > 0 && <span>v{d.version}</span>}
          </>
        }
        updated={at}
        actions={
          d.available && (
            <button type="button" className="cx-btn primary" disabled={d.providers.length >= d.max} title={d.providers.length >= d.max ? `At most ${d.max} providers` : undefined} onClick={() => void addProvider()}>
              Add provider…
            </button>
          )
        }
      />
      <Banners items={banners} />
      {!d.available && <Unavailable d={d} />}
      <Panel title="Providers" src={<Src>listSignInProviders · putSignInProvider · removeSignInProvider</Src>}>
        <Table rows={d.providers} />
      </Panel>
    </>
  )
}

function Unavailable({ d }: { d: ProviderList }) {
  const f = d.featureLevel
  return (
    <div className="cx-needs">
      <Glyph k="warn" />
      <span>
        <b title={d.reason}>Not available</b>
        {' '}· feature level <span className="mono">{f.active ?? '—'}</span> active, <span className="mono">{f.required}</span> required ·{' '}
        <Link to="/docs/operations/upgrades">Upgrades</Link>
      </span>
    </div>
  )
}

function Table({ rows }: { rows: ProviderView[] }) {
  const cols: Col<ProviderView>[] = [
    {
      id: 'p',
      label: 'Provider',
      sort: (a, b) => a.name.localeCompare(b.name),
      render: (p) => (
        <span className="cx-cellid">
          <ProviderIcon p={p} />
          <b>{p.name}</b>
          <span className="mono muted sm">{p.id}</span>
        </span>
      ),
    },
    { id: 'i', label: 'Issuer', className: 'trunc', style: { width: '100%', maxWidth: 0 }, render: (p) => <span className="mono sm" title={p.issuer}>{p.issuer}</span> },
    { id: 'a', label: 'Client auth', render: (p) => <span className="mono sm">{p.clientAuth.method}{p.clientAuth.signedSecret ? ' + signed' : ''}</span> },
    { id: 's', label: 'Sign-up', render: (p) => <Chip k={p.signup === 'off' ? 'idle' : 'info'}>{p.signup}</Chip> },
    { id: 'c', label: 'Sign-ups/day', r: true, render: (p) => (p.signup === 'off' ? <span className="muted">—</span> : <span className="mono" title="0 = no cap">{p.signupsPerDay === 0 ? '∞' : p.signupsPerDay}</span>) },
    { id: 'm', label: 'Response', render: (p) => <span className="mono sm">{p.responseMode}</span> },
    { id: 't', label: 'Trust email', render: (p) => <Chip k={p.trustEmail ? 'ok' : 'idle'}>{p.trustEmail ? 'yes' : 'no'}</Chip> },
    {
      id: 'u',
      label: 'Updated',
      sort: (a, b) => Date.parse(a.updatedAt) - Date.parse(b.updatedAt),
      render: (p) => (
        <span className="t2" title={p.updatedAt}>
          {ago(Date.parse(p.updatedAt))}
          {p.updatedBy && <span className="muted"> by {p.updatedBy}</span>}
        </span>
      ),
    },
  ]
  return (
    <DataTable
      rows={rows}
      cols={cols}
      rowKey={(p) => p.id}
      open={(p) => ({ type: 'provider', id: p.id })}
      label="Sign-in providers"
      empty={<Empty>No providers</Empty>}
    />
  )
}
