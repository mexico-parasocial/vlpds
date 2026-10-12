import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { confirmAction, FormDialog, openDialog } from '../../components/console/dialogs'
import { Banners, Chip, Empty, ErrorState, KV, Loading, NeedsVersion, PageHead, Panel, PanelBody, Sec, Spark, Src, Tiles, Toggle, type BannerSpec } from '../../components/console/kit'
import { DataTable } from '../../components/console/DataTable'
import { toast } from '../../components/console/toast'
import { listAccounts } from '../../lib/adminApi'
import { withAdmin } from '../../lib/console/adminAdapter'
import {
  baseVersion,
  diffDrafts,
  draftOf,
  freshOverride,
  lifetime,
  LEFT_OUT,
  LISTED_METHODS,
  METHOD_GROUPS,
  overrideQ,
  overrideRates,
  saveDraft,
  shortMethod,
  validDid,
  validMethod,
  validUrl,
  type AppviewOverride as Ov,
  type Draft,
  type Loaded,
  type SaveResult,
} from '../../lib/console/appviewOverride'
import { ago, fmtMs, fmtNum, fmtSi, plural } from '../../lib/console/fmt'
import { missing } from '../../lib/console/sys'
import { lastActor, rememberActor } from '../../lib/console/ratelimits'
import { errText } from '../../lib/xrpc'
import { DiffList } from './limitsUi'
import { AccountLink } from './peopleUi'

// AppView override: where chosen accounts' AppView calls go, which methods move, the traffic
// they make, and the config's history and nodes. Every edit is a diff saved with ifVersion
// (updateAppviewOverride), like the rate limits.

export function AppviewOverride() {
  const s = overrideQ.use()
  const d = s.data
  if (!d) {
    if (s.error && missing(s.error))
      return (
        <>
          <PageHead title="AppView override" />
          <Panel>
            <NeedsVersion what="The AppView override" nsid="vlpds.admin.getAppviewOverride" />
          </Panel>
        </>
      )
    return s.error ? <ErrorState error={s.error} retry={overrideQ.refresh} /> : <Loading label="Asking every node…" />
  }
  return <Page d={d} stale={!!s.error} />
}

// ---------------------------------------------------------------- editing

/** Starts from the config the server has now, applies `mutate`, shows the diff and saves it with ifVersion. */
export async function editOverride(o: { title: string; mutate: (x: Draft, d: Ov) => void; danger?: boolean; word?: string; what?: ReactNode; label?: (did: string) => string }): Promise<boolean> {
  let d: Ov
  try {
    d = await freshOverride()
  } catch (e) {
    toast(errText(e), { err: true })
    return false
  }
  const before = draftOf(d)
  const after: Draft = structuredClone(before)
  o.mutate(after, d)
  const diff = diffDrafts(before, after, o.label)
  if (!diff.length) {
    toast('Nothing to change')
    return false
  }
  const base = baseVersion(d)
  return confirmAction({
    tone: o.danger ? 'err' : 'warn',
    title: o.title,
    items: [
      <DiffList diff={diff} />,
      ...(o.what ? [o.what] : []),
      d.source === 'flags' && !d.config
        ? 'This writes the first config object. From then on it wins over the --appview-override flags on every node.'
        : `Saved as config v${base + 1}.`,
      `Every node applies it at once (the saving node nudges the others), or within ${d.refreshSecs} s.`,
    ],
    fields: [
      { id: 'actor', label: 'Your name (kept in the change history)', initial: lastActor() },
      { id: 'note', label: 'Note (optional): why this change' },
    ],
    word: o.word,
    primary: !o.danger,
    action: `Save as v${base + 1}`,
    call: `vlpds.admin.updateAppviewOverride {ifVersion: ${base}}`,
    run: async (v) => {
      rememberActor(String(v.actor ?? ''))
      return saveDraft(d, after, String(v.actor ?? ''), String(v.note ?? ''))
    },
    done: (r) => {
      const s = r as SaveResult
      const ok = s.nodes.filter((n) => n.ok).length
      return `Saved v${s.version}: ${ok} of ${s.nodes.length} ${s.nodes.length === 1 ? 'node' : 'nodes'} running it`
    },
  })
}

/** Adds an account to the override, or takes it off (the account page's toggle). */
export function toggleAccount(did: string, handle: string | undefined, on: boolean) {
  const who = handle ? `@${handle}` : did
  return editOverride({
    title: on ? `Send ${who}’s AppView calls to the override?` : `Take ${who} off the override?`,
    label: (x) => (x === did && handle ? `@${handle} · ${did}` : x),
    what: on ? 'Only the methods the override lists move. Everything else stays on the default AppView.' : 'Every AppView call of this account goes to the default AppView again.',
    mutate: (x) => {
      x.accounts = on ? [...new Set([...x.accounts, did])] : x.accounts.filter((a) => a !== did)
    },
  })
}

export function setEnabled(on: boolean) {
  return editOverride({
    title: on ? 'Turn the AppView override on?' : 'Turn the AppView override off?',
    danger: !on,
    what: on ? 'The listed accounts’ listed methods go to the override again.' : 'Every call goes to the default AppView. The accounts and methods are kept.',
    mutate: (x) => void (x.enabled = on),
  })
}

function targetDialog(d: Ov) {
  openDialog((close) => <TargetForm close={close} d={d} />)
}

function TargetForm({ close, d }: { close: () => void; d: Ov }) {
  const cur = draftOf(d)
  const [url, setUrl] = useState(cur.url)
  const [did, setDid] = useState(cur.did)
  const u = url.trim()
  const s = did.trim()
  const badUrl = !!u && !validUrl(u)
  const badDid = !!s && !validDid(s)
  const ok = !!u && !!s && !badUrl && !badDid && (u !== cur.url || s !== cur.did)
  return (
    <FormDialog
      title="Where the override sends calls"
      icon="→"
      call="reviewed as a diff, then vlpds.admin.updateAppviewOverride"
      action="Review…"
      disabled={!ok}
      onCancel={close}
      onSubmit={() => {
        close()
        editOverride({
          title: 'Change the override AppView?',
          mutate: (x) => {
            x.url = u
            x.did = s
          },
        })
      }}
    >
      <div>
        <label className="cx-lbl" htmlFor="ov-url">
          Base URL
        </label>
        <input id="ov-url" className="cx-inp mono" autoFocus value={url} onChange={(e) => setUrl(e.target.value)} placeholder="http://appview.internal:2584" spellCheck={false} aria-invalid={badUrl || undefined} />
        {badUrl && <div className="s-err sm" style={{ marginTop: 4 }}>An http:// or https:// URL with a host.</div>}
      </div>
      <div>
        <label className="cx-lbl" htmlFor="ov-did">
          Service DID (the service-auth audience, never resolved)
        </label>
        <input id="ov-did" className="cx-inp mono" value={did} onChange={(e) => setDid(e.target.value)} placeholder="did:web:appview.example" spellCheck={false} aria-invalid={badDid || undefined} />
        {badDid && <div className="s-err sm" style={{ marginTop: 4 }}>Not a DID.</div>}
      </div>
      <p className="muted sm" style={{ margin: 0 }}>
        The URL is trusted like <span className="mono">--appview</span>: a private or tailnet address is fine. Calls carry a service-auth token for the account with this DID as its
        audience.
      </p>
    </FormDialog>
  )
}

function addAccountDialog() {
  openDialog((close) => <AddAccount close={close} />)
}

function AddAccount({ close }: { close: () => void }) {
  const [q, setQ] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<unknown>()
  const v = q.trim().replace(/^@/, '')
  const submit = async () => {
    setBusy(true)
    setError(undefined)
    try {
      const r = await withAdmin((c) => listAccounts(c, { q: v, limit: 5 }))
      const lv = v.toLowerCase()
      const hit = r.accounts.find((a) => a.did === v || a.handle.toLowerCase() === lv)
      if (!hit) {
        setError(new Error(`No account on this PDS is ${v.startsWith('did:') ? '' : '@'}${v}.`))
        return
      }
      close()
      toggleAccount(hit.did, hit.handle, true)
    } catch (e) {
      setError(e)
    } finally {
      setBusy(false)
    }
  }
  return (
    <FormDialog title="Add an account to the override" call="looked up with listAccounts, then reviewed as a diff" action="Review…" disabled={!v || busy} busy={busy} error={error} onCancel={close} onSubmit={() => void submit()}>
      <div>
        <label className="cx-lbl" htmlFor="ov-acct">
          Handle or DID of an account on this PDS
        </label>
        <input id="ov-acct" className="cx-inp mono" autoFocus value={q} onChange={(e) => setQ(e.target.value)} placeholder="alice.example.com or did:plc:…" spellCheck={false} />
      </div>
      <p className="muted sm" style={{ margin: 0 }}>
        Only its AppView calls for the listed methods move. You can also turn this on from the account’s page.
      </p>
    </FormDialog>
  )
}

// ---------------------------------------------------------------- the page

function Page({ d, stale }: { d: Loaded; stale: boolean }) {
  const cur = draftOf(d)
  const r = overrideRates()
  const life = lifetime(d.stats)
  const reachable = d.nodes.filter((n) => n.reachable && !n.unsupported)
  const versions = [...new Set(reachable.map((n) => `${n.source}:${n.configVersion}`))]
  const onVersion = reachable.filter((n) => n.configVersion === d.configVersion && n.source === d.source).length
  const active = !!d.effective
  const last = <T,>(xs: T[]) => xs[xs.length - 1]

  const banners: BannerSpec[] = []
  if (stale) banners.push({ id: 'stale', tone: 'warn', title: 'Not updating', desc: 'getAppviewOverride is failing; showing the last answer.' })
  for (const n of d.nodes.filter((n) => n.configError))
    banners.push({
      id: `err-${n.node}`,
      tone: 'err',
      title: `${n.node} rejected config${n.configError!.version != null ? ` v${n.configError!.version}` : ''}`,
      desc: `${ago(n.configError!.atMs)}; it keeps running v${n.configVersion}`,
      body: <span className="mono sm">{n.configError!.message}</span>,
      open: true,
    })
  if (versions.length > 1)
    banners.push({ id: 'versions', tone: 'warn', title: 'Nodes disagree on the config', desc: `${reachable.map((n) => `${n.node} ${n.source === 'object' ? `v${n.configVersion}` : 'flags'}`).join(' · ')} · they re-read every ${d.refreshSecs} s` })
  const noDefault = reachable.filter((n) => n.appviewConfigured === false)
  if (noDefault.length && cur.accounts.length)
    banners.push({ id: 'noav', tone: 'warn', title: `${noDefault.map((n) => n.node).join(', ')} ${noDefault.length === 1 ? 'has' : 'have'} no --appview`, desc: 'the override only moves calls bound for the default AppView, so nothing moves there' })
  const old = d.nodes.filter((n) => n.unsupported)
  if (old.length) banners.push({ id: 'old', tone: 'warn', title: `${old.map((n) => n.node).join(', ')} run an older vlpds`, desc: 'they don’t read this config: the flags they started with stay in force there' })
  if (d.unreachableNodes?.length) banners.push({ id: 'unreach', tone: 'warn', title: `${plural(d.unreachableNodes.length, 'node')} didn’t answer`, desc: `${d.unreachableNodes.join(', ')}: their traffic is missing` })
  if (d.source === 'flags' && d.flags)
    banners.push({ id: 'flags', tone: 'info', title: 'In force from the --appview-override flags', desc: 'The first save here writes the config object to the bucket. From then on it wins over the flags on every node.' })
  if (!cur.enabled && d.config) banners.push({ id: 'off', tone: 'warn', title: 'The override is off', desc: 'every call goes to the default AppView; the accounts and methods are kept', right: <button type="button" className="cx-btn sm" onClick={() => setEnabled(true)}>Turn on…</button> })

  const sourceText = d.source === 'object' ? (
    <>
      config v{d.configVersion} in <span className="mono">config/appview_override.json</span>
    </>
  ) : d.flags ? (
    'the --appview-override flags (no config saved yet)'
  ) : (
    'off: no flags, no config saved yet'
  )

  return (
    <>
      <PageHead
        title="AppView override"
        sub={
          <>
            <span>{sourceText}</span>
            <span>
              applied on {onVersion}/{reachable.length} {reachable.length === 1 ? 'node' : 'nodes'} · re-read every {d.refreshSecs} s
            </span>
          </>
        }
        updated={d.fetchedAt}
        actions={
          <span className="cx-cellid sm t2">
            Override <Toggle on={cur.enabled && (!!d.config || !!d.flags)} label={cur.enabled ? 'Turn the override off' : 'Turn the override on'} onChange={(on) => setEnabled(on)} />
          </span>
        }
      />
      <Banners items={banners} />
      <Tiles
        boxed
        tiles={[
          { label: 'Requests', right: 'to the override', value: fmtSi(last(r.requests) ?? 0), unit: '/s', sec: `${fmtNum(life.requests)} total`, spark: <Spark data={r.requests} min={0.05} /> },
          { label: 'Fallbacks', right: 'to the default', value: fmtSi(last(r.fallbacks) ?? 0), unit: '/s', sec: `${fmtNum(d.stats.fallbacks)} total`, spark: <Spark data={r.fallbacks} color="amber" min={0.05} /> },
          {
            label: 'Errors',
            right: '5xx, unreachable',
            value: fmtSi(last(r.errors) ?? 0),
            unit: '/s',
            sec: `${fmtNum(life.errors)} total · ${fmtNum(life.clientErrors)} 4xx`,
            spark: <Spark data={r.errors} color="warn" min={0.05} />,
          },
          { label: 'Response head', right: 'mean time', value: fmtMs(last(r.headMs) ?? life.meanHeadMs), spark: <Spark data={r.headMs} color="c3" /> },
        ]}
      />
      <div className="cx-grid2 cx-mb">
        <div className="cx-stack">
          <Target d={d} cur={cur} active={active} />
          <Accounts d={d} cur={cur} />
        </div>
        <Methods d={d} cur={cur} />
      </div>
      <div className="cx-grid2">
        <History d={d} />
        <Nodes d={d} />
      </div>
    </>
  )
}

function Target({ d, cur, active }: { d: Loaded; cur: Draft; active: boolean }) {
  return (
    <Panel
      title="Target"
      src={<Src>getAppviewOverride · updateAppviewOverride</Src>}
      right={active ? <Chip k="ok">moving calls</Chip> : <Chip k="idle">moving nothing</Chip>}
      foot="Only calls bound for the default AppView move: no atproto-proxy header, or one naming its DID. Chat, labelers and feed generators are never touched."
    >
      <PanelBody>
        <KV
          rows={[
            [
              'Override URL',
              cur.url ? <span className="mono sm">{cur.url}</span> : <span className="muted">not set</span>,
              {
                act: (
                  <button type="button" className="cx-btn sm" onClick={() => targetDialog(d)}>
                    Edit…
                  </button>
                ),
              },
            ],
            ['Service DID', cur.did ? <span className="mono sm">{cur.did}</span> : <span className="muted">not set</span>],
            [
              'Fallback',
              cur.fallback ? 'a GET it answers MethodNotImplemented goes to the default' : 'its answer goes to the app as is',
              {
                act: (
                  <Toggle
                    on={cur.fallback}
                    label={cur.fallback ? 'Turn the fallback off' : 'Turn the fallback on'}
                    onChange={(on) => editOverride({ title: on ? 'Fall back to the default AppView?' : 'Stop falling back?', mutate: (x) => void (x.fallback = on) })}
                  />
                ),
              },
            ],
            [
              'Default AppView',
              d.defaultAppview ? (
                <span className="mono sm">
                  {d.defaultAppview.url} <span className="muted">· {d.defaultAppview.did}</span>
                </span>
              ) : (
                <span className="muted">none (--appview unset)</span>
              ),
            ],
          ]}
        />
      </PanelBody>
    </Panel>
  )
}

function Accounts({ d, cur }: { d: Loaded; cur: Draft }) {
  const rows = [...cur.accounts].sort()
  return (
    <Panel
      title="Accounts"
      right={
        <>
          <span className="muted sm">
            {rows.length} of at most {fmtNum(d.limits.accounts)}
          </span>
          <button type="button" className="cx-btn sm" onClick={addAccountDialog}>
            Add account…
          </button>
        </>
      }
      foot="Every other account stays on the default AppView."
    >
      <DataTable
        compact
        rows={rows}
        rowKey={(did) => did}
        open={(did) => ({ type: 'account', id: did })}
        label="Accounts"
        empty={<Empty>No accounts: the override moves nothing. Add one here or from an account’s page.</Empty>}
        cols={[
          { id: 'who', label: 'Account', render: (did) => <AccountLink did={did} /> },
          { id: 'did', label: 'DID', style: { width: '100%' }, render: (did) => <span className="mono sm t2">{did}</span> },
          {
            id: 'act',
            label: '',
            r: true,
            render: (did) => (
              <button
                type="button"
                className="cx-btn sm"
                onClick={(e) => {
                  e.stopPropagation()
                  toggleAccount(did, undefined, false)
                }}
              >
                Remove…
              </button>
            ),
          },
        ]}
      />
    </Panel>
  )
}

function Methods({ d, cur }: { d: Loaded; cur: Draft }) {
  const saved = useMemo(() => new Set(cur.methods), [cur.methods.join(',')])
  const [sel, setSel] = useState<Set<string>>(saved)
  const [extra, setExtra] = useState('')
  useEffect(() => setSel(saved), [saved])
  const others = [...new Set([...cur.methods, ...sel])].filter((m) => !LISTED_METHODS.has(m)).sort()
  const added = [...sel].filter((m) => !saved.has(m))
  const removed = [...saved].filter((m) => !sel.has(m))
  const dirty = added.length + removed.length > 0
  const flip = (m: string) =>
    setSel((s) => {
      const n = new Set(s)
      if (n.has(m)) n.delete(m)
      else n.add(m)
      return n
    })
  const x = extra.trim()
  const badExtra = !!x && !validMethod(x)
  const box = (m: string) => (
    <label key={m} className={saved.has(m) !== sel.has(m) ? 'changed' : undefined} title={m}>
      <input type="checkbox" checked={sel.has(m)} onChange={() => flip(m)} />
      <span>{shortMethod(m)}</span>
    </label>
  )
  return (
    <Panel
      title="Methods"
      right={<span className="muted sm">{plural(saved.size, 'method')} move</span>}
      foot={
        <span>
          Not listed here: {LEFT_OUT}. They stay on the default AppView unless you add them by name. Procedures (POST) are never resent to the default.
        </span>
      }
    >
      <PanelBody>
        <div className="cxp-methods">
          {METHOD_GROUPS.map(([g, ms]) => (
            <fieldset key={g}>
              <legend>{g}</legend>
              {ms.map(box)}
            </fieldset>
          ))}
          {others.length > 0 && (
            <fieldset>
              <legend>Added by name</legend>
              {others.map(box)}
            </fieldset>
          )}
        </div>
        <div className="cx-form-row" style={{ marginTop: 10 }}>
          <input
            className="cx-inp mono"
            value={extra}
            onChange={(e) => setExtra(e.target.value)}
            placeholder="another app.bsky method, e.g. app.bsky.feed.searchPosts"
            aria-label="Another method"
            spellCheck={false}
            aria-invalid={badExtra || undefined}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && x && !badExtra) {
                setSel((s) => new Set([...s, x]))
                setExtra('')
              }
            }}
          />
          <button
            type="button"
            className="cx-btn sm"
            disabled={!x || badExtra}
            onClick={() => {
              setSel((s) => new Set([...s, x]))
              setExtra('')
            }}
          >
            Add
          </button>
          <span style={{ flex: '1 1 0' }} />
          <button type="button" className="cx-btn sm" disabled={!dirty} onClick={() => setSel(saved)}>
            Reset
          </button>
          <button
            type="button"
            className="cx-btn sm primary"
            disabled={!dirty}
            onClick={() =>
              editOverride({
                title: 'Change which methods move?',
                mutate: (dr) => void (dr.methods = [...sel].sort()),
              })
            }
          >
            {dirty ? `Save ${[added.length ? `+${added.length}` : '', removed.length ? `−${removed.length}` : ''].filter(Boolean).join(' ')}…` : 'Save…'}
          </button>
        </div>
        {badExtra && <div className="s-err sm" style={{ marginTop: 4 }}>An exact app.bsky.* method name, no wildcards.</div>}
        {d.limits.methods <= sel.size && <div className="s-warn sm">At most {d.limits.methods} methods.</div>}
      </PanelBody>
    </Panel>
  )
}

function History({ d }: { d: Loaded }) {
  const h = [...(d.config?.history ?? [])].reverse()
  const latest = h[0]
  return (
    <Sec title="Change history" digest={latest ? `v${latest.version} by ${latest.by}, ${ago(Date.parse(latest.at))}` : 'no config saved yet'} open flush>
      {h.length ? (
        <div className="cx-tw" style={{ maxHeight: 360 }}>
          <table className="cx-t compact">
            <tbody>
              {h.map((a) => (
                <tr key={a.version}>
                  <td className="mono" style={{ verticalAlign: 'top' }}>
                    v{a.version}
                  </td>
                  <td style={{ verticalAlign: 'top' }} title={a.at}>
                    {ago(Date.parse(a.at))}
                  </td>
                  <td style={{ verticalAlign: 'top' }}>
                    {a.by}
                    <div className="muted mono sm">{a.node}</div>
                  </td>
                  <td className="wrap t2">
                    {a.changes.map((x, i) => (
                      <div key={i} className="mono sm">
                        {x}
                      </div>
                    ))}
                    {a.note && <div className="sm">“{a.note}”</div>}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div className="cx-empty">{d.flags ? 'No changes yet: the flags are in force.' : 'No changes yet: the override is off.'}</div>
      )}
    </Sec>
  )
}

function Nodes({ d }: { d: Loaded }) {
  return (
    <Sec title="Nodes" digest={`each re-reads the config every ${d.refreshSecs} s`} open flush>
      <div className="cx-tw">
        <table className="cx-t compact">
          <thead>
            <tr>
              <th>Node</th>
              <th>Config</th>
              <th className="r">Accounts</th>
              <th className="r">Requests</th>
              <th className="r">Fallbacks</th>
              <th className="r">Applied</th>
            </tr>
          </thead>
          <tbody>
            {d.nodes.map((n) => {
              let c: ReactNode
              if (!n.reachable) c = <Chip k="err">unreachable</Chip>
              else if (n.unsupported) c = <Chip k="warn">older vlpds</Chip>
              else if (n.configError) c = <Chip k="err">v{n.configVersion}, rejected newer</Chip>
              else if (n.source === 'flags') c = <Chip k="plain">flags</Chip>
              else c = <Chip k={n.configVersion === d.configVersion ? 'ok' : 'warn'}>v{n.configVersion}</Chip>
              const st = n.stats
              return (
                <tr key={n.node}>
                  <td className="mono">
                    {n.node}
                    {n.self && <span className="muted"> · this</span>}
                  </td>
                  <td>
                    {c}
                    {n.reachable && !n.unsupported && !n.active && <Chip k="idle">moving nothing</Chip>}
                    {n.appviewConfigured === false && <Chip k="warn">no --appview</Chip>}
                  </td>
                  <td className="r">{n.accounts ?? '—'}</td>
                  <td className="r mono">{st ? fmtNum(st.ok + st.clientError + st.serverError + st.unreachable) : '—'}</td>
                  <td className="r mono">{st ? fmtNum(st.fallbacks) : '—'}</td>
                  <td className="r muted">{n.loadedAtMs ? ago(n.loadedAtMs) : '—'}</td>
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>
    </Sec>
  )
}
