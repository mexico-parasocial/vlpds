import { useRef, useState, type ReactNode } from 'react'
import { confirmAction, FormDialog, openDialog } from '../../components/console/dialogs'
import { registerDetail } from '../../components/console/Drawer'
import { Chip, Copy, Glyph, KV, Sec, Seg, Src, Strip, Toggle } from '../../components/console/kit'
import { panelParam } from '../../components/console/nav'
import { registerPalette } from '../../components/console/Palette'
import { toast } from '../../components/console/toast'
import { clusterQ } from '../../lib/console/cluster'
import { ago, plural } from '../../lib/console/fmt'
import { lastActor, rememberActor } from '../../lib/console/ratelimits'
import {
  blankDraft,
  DEFAULT_ALGS,
  draftOf,
  hasErrors,
  ICON_MAX,
  ID_ALGS,
  iconBytes,
  METHODS,
  providersQ,
  putProvider,
  removeProvider,
  storedText,
  usesKey,
  usesSecret,
  validate,
  type ClientMethod,
  type Cred,
  type Draft,
  type NodeResult,
  type ProviderList,
  type ProviderView,
  type ResponseMode,
  type Saved,
  type Signup,
  type Stored,
} from '../../lib/console/signInProviders'
import { navigate } from '../../lib/router'
import { errText, XrpcError } from '../../lib/xrpc'

// Sign-in providers: the add/edit form, removal, the per-node results, the slide-over and ⌘K
// entries. Every write carries the list's version (ifVersion) read just before the form opens.

export const SIGNUP_LABEL: Record<Signup, string> = { off: 'off', invite: 'invite', open: 'open' }

export function ProviderIcon({ p }: { p: Pick<ProviderView, 'icon' | 'name'> }) {
  return p.icon ? <img className="cxs-icon" src={p.icon} alt="" /> : <span className="cxs-icon none" aria-hidden="true">{p.name.slice(0, 1).toUpperCase()}</span>
}

export const callbackFor = (id: string, existing?: ProviderView) =>
  existing && id === existing.id ? existing.callbackUrl : `${(clusterQ.get().data?.publicUrl ?? location.origin).replace(/\/+$/, '')}/oauth/federated/callback/${id}`

const fresh = async () => {
  try {
    return await providersQ.fresh()
  } catch (e) {
    toast(errText(e), { err: true })
    return undefined
  }
}

export async function addProvider() {
  const d = await fresh()
  if (!d) return
  if (d.providers.length >= d.max) return toast(`At most ${d.max} providers`, { err: true })
  openDialog((close) => <ProviderForm list={d} close={close} />)
}

export async function editProvider(id: string) {
  const d = await fresh()
  const p = d?.providers.find((x) => x.id === id)
  if (!d) return
  if (!p) return toast(`${id} is gone`, { err: true })
  openDialog((close) => <ProviderForm list={d} existing={p} close={close} />)
}

// ---------------------------------------------------------------- the form

function Field({ id, label, err, title, children }: { id: string; label: ReactNode; err?: string; title?: string; children: ReactNode }) {
  return (
    <div>
      <label className="cx-lbl" htmlFor={id} title={title}>
        {label}
      </label>
      {children}
      {err && <div className="cxs-bad">{err}</div>}
    </div>
  )
}

function CredField({ id, label, what, stored, cred, onChange, err }: { id: string; label: string; what: 'secret' | 'key'; stored: Stored; cred: Cred; onChange: (c: Cred) => void; err?: string }) {
  const modes: { v: Cred['mode']; label: string }[] = [...(stored ? [{ v: 'keep' as const, label: 'keep' }] : []), { v: 'value', label: 'new value' }]
  return (
    <div className="cxs-cred">
      <div className="top">
        <label className="cx-lbl" htmlFor={id} style={{ margin: 0 }}>
          {label}
        </label>
        <Chip k={stored ? 'ok' : 'idle'} title={stored ? 'Never shown again. Keep it, or enter a new one' : 'Nothing stored'}>
          {storedText(stored)}
        </Chip>
        <span style={{ marginLeft: 'auto' }}>
          <Seg<Cred['mode']> label={`${label} source`} value={cred.mode} onChange={(mode) => onChange({ mode, text: '' })} options={modes} />
        </span>
      </div>
      {cred.mode === 'value' &&
        (what === 'key' ? (
          <textarea id={id} className="cx-inp mono" rows={4} value={cred.text} onChange={(e) => onChange({ ...cred, text: e.target.value })} placeholder="PKCS#8 PEM, P-256" spellCheck={false} aria-invalid={!!err || undefined} />
        ) : (
          <input id={id} className="cx-inp mono" type="password" autoComplete="new-password" value={cred.text} onChange={(e) => onChange({ ...cred, text: e.target.value })} aria-invalid={!!err || undefined} />
        ))}
      {err && <div className="cxs-bad">{err}</div>}
    </div>
  )
}

function readIcon(f: File): Promise<string> {
  return new Promise((res, rej) => {
    const r = new FileReader()
    r.onload = () => res(String(r.result))
    r.onerror = () => rej(r.error ?? new Error('Couldn’t read the file'))
    r.readAsDataURL(f)
  })
}

type Failure = { kind: 'unreachable' | 'version' | 'other'; error: unknown }

function ProviderForm({ list, existing, close }: { list: ProviderList; existing?: ProviderView; close: () => void }) {
  const [d, setD] = useState<Draft>(() => (existing ? draftOf(existing) : blankDraft()))
  const [actor, setActor] = useState(lastActor())
  const [busy, setBusy] = useState(false)
  const [fail, setFail] = useState<Failure>()
  const [touched, setTouched] = useState(false)
  const file = useRef<HTMLInputElement>(null)
  const set = (p: Partial<Draft>) => setD((x) => ({ ...x, ...p }))
  const errs = validate(d, { existing, count: list.providers.length, max: list.max })
  const show = (k: keyof typeof errs) => (touched ? errs[k] : undefined)
  const algs = d.algs

  const submit = async (force: boolean) => {
    setTouched(true)
    if (hasErrors(errs)) return
    setBusy(true)
    setFail(undefined)
    try {
      rememberActor(actor)
      const r = await putProvider(d, { ifVersion: list.version, force, actor })
      close()
      showResults(`Saved ${d.name.trim()}`, r)
    } catch (e) {
      if (e instanceof XrpcError && e.error === 'ProviderUnreachable') setFail({ kind: 'unreachable', error: e })
      else if (e instanceof XrpcError && e.error === 'VersionMismatch') setFail({ kind: 'version', error: e })
      else setFail({ kind: 'other', error: e })
    } finally {
      setBusy(false)
    }
  }

  const pickIcon = async (f?: File) => {
    if (!f) return
    try {
      set({ icon: await readIcon(f) })
    } catch (e) {
      setFail({ kind: 'other', error: e })
    }
  }

  return (
    <FormDialog
      wide
      title={existing ? `Edit ${existing.name}` : 'Add a sign-in provider'}
      icon={existing ? '✎' : '+'}
      call={`vlpds.admin.putSignInProvider {ifVersion: ${list.version}}`}
      action={existing ? 'Save' : 'Add provider'}
      busy={busy}
      onCancel={close}
      onSubmit={() => void submit(false)}
    >
      <div className="cxs-form">
        <div className="cxs-row">
          <Field id="sp-name" label="Name" err={show('name')} title="The sign-in button’s text">
            <input id="sp-name" className="cx-inp" autoFocus value={d.name} onChange={(e) => set({ name: e.target.value })} placeholder="Google" />
          </Field>
          <Field id="sp-id" label="ID" err={show('id')} title="a–z, 0–9 and -, up to 32. Renaming it changes no identity">
            <input id="sp-id" className="cx-inp mono" value={d.id} disabled={!!existing} onChange={(e) => set({ id: e.target.value.trim() })} placeholder="google" spellCheck={false} autoCapitalize="none" />
          </Field>
        </div>
        <Field id="sp-cb" label="Callback URL" title="Register this at the provider. It depends on the ID">
          <Copy text={callbackFor(d.id.trim() || '{id}', existing)} full />
        </Field>
        <Field id="sp-issuer" label="Issuer" err={show('issuer')} title="Discovery’s issuer must equal it exactly. One provider per issuer">
          <input id="sp-issuer" className="cx-inp mono" value={d.issuer} onChange={(e) => set({ issuer: e.target.value })} placeholder="https://accounts.google.com" spellCheck={false} autoCapitalize="none" />
        </Field>
        <Field id="sp-client" label="Client ID" err={show('clientId')}>
          <input id="sp-client" className="cx-inp mono" value={d.clientId} onChange={(e) => set({ clientId: e.target.value })} spellCheck={false} autoCapitalize="none" />
        </Field>
        <div>
          <div className="cx-lbl" title={`A data:image/ URI of at most ${ICON_MAX / 1024} KiB`}>
            Icon
          </div>
          <div className="cx-form-row">
            {d.icon && <ProviderIcon p={{ icon: d.icon, name: d.name || '?' }} />}
            {d.icon && <span className="mono sm t2">{(iconBytes(d.icon) / 1024).toFixed(1)} KiB</span>}
            <input ref={file} type="file" accept="image/png,image/svg+xml,image/webp,image/jpeg,image/gif" hidden onChange={(e) => void pickIcon(e.target.files?.[0])} />
            <button type="button" className="cx-btn sm" onClick={() => file.current?.click()}>
              {d.icon ? 'Replace…' : 'Choose…'}
            </button>
            {d.icon && (
              <button type="button" className="cx-btn sm" onClick={() => set({ icon: null })}>
                Remove
              </button>
            )}
          </div>
          {show('icon') && <div className="cxs-bad">{errs.icon}</div>}
        </div>

        <div className="cxs-h">Client auth</div>
        <Seg<ClientMethod> label="Client auth method" value={d.method} onChange={(method) => set({ method })} options={METHODS.map((m) => ({ v: m, label: m }))} />
        {usesSecret(d) && <CredField id="sp-secret" label="Client secret" what="secret" stored={existing && existing.clientAuth.method !== 'private_key_jwt' ? existing.clientAuth.secret : null} cred={d.secret} onChange={(secret) => set({ secret })} err={show('secret')} />}
        {usesKey(d) && (
          <>
            <CredField id="sp-key" label="Private key" what="key" stored={existing?.clientAuth.key ?? null} cred={d.key} onChange={(key) => set({ key })} err={show('key')} />
            <Field id="sp-keyid" label="Key ID" err={show('keyId')}>
              <input id="sp-keyid" className="cx-inp mono" value={d.keyId} onChange={(e) => set({ keyId: e.target.value })} spellCheck={false} autoCapitalize="none" />
            </Field>
          </>
        )}
        {d.method !== 'private_key_jwt' && (
          <label className="cx-form-row" style={{ gap: 8 }} title="The client secret is a JWT the PDS signs with the key, and refreshes before it expires">
            <Toggle on={d.signed} label="Signed secret" onChange={(signed) => set({ signed })} /> Signed secret
          </label>
        )}
        {d.signed && (
          <div className="cxs-row">
            <Field id="sp-ss-iss" label="Issuer (team id)" err={show('signedIssuer')}>
              <input id="sp-ss-iss" className="cx-inp mono" value={d.signedIssuer} onChange={(e) => set({ signedIssuer: e.target.value })} spellCheck={false} />
            </Field>
            <Field id="sp-ss-aud" label="Audience" title="Blank: the provider’s default">
              <input id="sp-ss-aud" className="cx-inp mono" value={d.signedAudience} onChange={(e) => set({ signedAudience: e.target.value })} spellCheck={false} />
            </Field>
            <Field id="sp-ss-life" label="Lifetime (s)" err={show('signedLifetime')} title="Blank: the server’s default">
              <input id="sp-ss-life" className="cx-inp mono" inputMode="numeric" value={d.signedLifetime} onChange={(e) => set({ signedLifetime: e.target.value })} placeholder="3600" />
            </Field>
          </div>
        )}

        <div className="cxs-h">Sign-in</div>
        <Field id="sp-scopes" label="Scopes" title="openid is always sent. Blank: openid email">
          <input id="sp-scopes" className="cx-inp mono" value={d.scopes} onChange={(e) => set({ scopes: e.target.value })} placeholder="openid email" spellCheck={false} autoCapitalize="none" />
        </Field>
        <div className="cx-form-row" style={{ gap: 18 }}>
          <div>
            <div className="cx-lbl">Response mode</div>
            <Seg<ResponseMode> label="Response mode" value={d.responseMode} onChange={(responseMode) => set({ responseMode })} options={[{ v: 'query', label: 'query' }, { v: 'form_post', label: 'form_post' }]} />
          </div>
          <div>
            <div className="cx-lbl" title="What an identity no account uses may do: off refuses it, invite needs a code, open creates an account">
              Sign-up
            </div>
            <Seg<Signup> label="Sign-up" value={d.signup} onChange={(signup) => set({ signup })} options={(Object.keys(SIGNUP_LABEL) as Signup[]).map((v) => ({ v, label: SIGNUP_LABEL[v] }))} />
          </div>
          <div>
            <label className="cx-lbl" htmlFor="sp-cap" title="Accounts created through this provider per UTC day, cluster-wide. 0 = no cap. Blank: 100">
              Sign-ups per day
            </label>
            <input id="sp-cap" className="cx-inp mono" style={{ width: 110 }} inputMode="numeric" disabled={d.signup === 'off'} value={d.signupsPerDay} onChange={(e) => set({ signupsPerDay: e.target.value })} placeholder="100" />
            {show('signupsPerDay') && <div className="cxs-bad">{errs.signupsPerDay}</div>}
          </div>
          <label className="cx-form-row" style={{ gap: 8 }} title="Use the provider’s email_verified to confirm the email at sign-up. Never used to find or link an account">
            <Toggle on={d.trustEmail} label="Trust email" onChange={(trustEmail) => set({ trustEmail })} /> Trust email
          </label>
        </div>

        <div className="cxs-h">Admit only</div>
        <div>
          <div className="cx-lbl" title="Exact-match claims in the id_token, such as hd = example.com">
            Required claims
          </div>
          <div className="cxs-claims">
            {d.claims.map((c, i) => (
              <div key={i}>
                <input className="cx-inp mono" aria-label="Claim" value={c.k} placeholder="hd" spellCheck={false} autoCapitalize="none" onChange={(e) => set({ claims: d.claims.map((x, j) => (j === i ? { ...x, k: e.target.value } : x)) })} />
                <input className="cx-inp mono" aria-label="Value" value={c.v} placeholder="example.com" spellCheck={false} autoCapitalize="none" onChange={(e) => set({ claims: d.claims.map((x, j) => (j === i ? { ...x, v: e.target.value } : x)) })} />
                <button type="button" className="cx-btn sm" aria-label="Remove claim" onClick={() => set({ claims: d.claims.filter((_, j) => j !== i) })}>
                  ×
                </button>
              </div>
            ))}
            <div>
              <button type="button" className="cx-btn sm" onClick={() => set({ claims: [...d.claims, { k: '', v: '' }] })}>
                Add claim
              </button>
            </div>
          </div>
          {show('claims') && <div className="cxs-bad">{errs.claims}</div>}
        </div>
        <div>
          <div className="cx-lbl" title={`ID token algorithms accepted. None checked: ${DEFAULT_ALGS.join(' ')}`}>
            ID token algorithms
          </div>
          <div className="cxs-algs" role="group" aria-label="ID token algorithms">
            {ID_ALGS.map((a) => (
              <label key={a}>
                <input type="checkbox" checked={algs.includes(a)} onChange={(e) => set({ algs: e.target.checked ? ID_ALGS.filter((x) => x === a || algs.includes(x)) : algs.filter((x) => x !== a) })} /> {a}
              </label>
            ))}
          </div>
        </div>

        <div className="cxs-h">Audit</div>
        <Field id="sp-actor" label="Your name (kept in the audit log)">
          <input id="sp-actor" className="cx-inp" value={actor} onChange={(e) => setActor(e.target.value)} />
        </Field>
        {errs.max && <div className="cxs-bad">{errs.max}</div>}
        {fail && <FailureView fail={fail} busy={busy} retry={() => void submit(true)} reload={() => { void providersQ.refresh(); close() }} base={list.version} />}
      </div>
    </FormDialog>
  )
}

function FailureView({ fail, busy, retry, reload, base }: { fail: Failure; busy: boolean; retry: () => void; reload: () => void; base: number }) {
  return (
    <div className="cxs-err" role="alert">
      <div className="row">
        <span className="cx-g">■</span>
        <span>
          {fail.kind === 'version' ? `The providers changed since you opened this (it was v${base}). Reload, then make the change again.` : errText(fail.error)}
        </span>
      </div>
      {fail.kind === 'unreachable' && (
        <div className="acts">
          <button type="button" className="cx-btn sm" disabled={busy} onClick={retry} title="Saves without fetching the issuer’s discovery document">
            Save without checking issuer
          </button>
        </div>
      )}
      {fail.kind === 'version' && (
        <div className="acts">
          <button type="button" className="cx-btn sm" onClick={reload}>
            Reload providers
          </button>
        </div>
      )}
    </div>
  )
}

// ---------------------------------------------------------------- results and removal

export function showResults(title: string, r: Saved) {
  openDialog((close) => <Results title={title} r={r} close={close} />)
}

export const nodeSummary = (nodes: NodeResult[]) => `${nodes.filter((n) => n.ok).length} of ${plural(nodes.length, 'node')}`

function Results({ title, r, close }: { title: string; r: Saved; close: () => void }) {
  const bad = r.nodes.filter((n) => !n.ok)
  return (
    <FormDialog title={title} icon={bad.length ? '▲' : '✓'} action="Done" call={`v${r.version} · ${nodeSummary(r.nodes)} told`} onSubmit={close} onCancel={close}>
      <div className="cxs-nodes">
        {r.nodes.map((n) => (
          <div key={n.node}>
            <Glyph k={n.ok ? 'ok' : 'err'} />
            <b className="mono">{n.node}</b>
            {n.error && <span className="t2">{n.error}</span>}
          </div>
        ))}
      </div>
    </FormDialog>
  )
}

export async function removeProviderDialog(id: string) {
  const d = await fresh()
  const p = d?.providers.find((x) => x.id === id)
  if (!d) return
  if (!p) return toast(`${id} is gone`, { err: true })
  return confirmAction({
    tone: 'err',
    title: `Remove ${p.name}?`,
    items: ['Its sign-in button goes away on every node.', 'Identities linked through it stay on their accounts, dormant, and can’t sign in.'],
    fields: [{ id: 'actor', label: 'Your name (kept in the audit log)', initial: lastActor() }],
    word: p.id,
    action: 'Remove provider',
    call: `vlpds.admin.removeSignInProvider {ifVersion: ${d.version}}`,
    run: async (v) => {
      rememberActor(String(v.actor ?? ''))
      const r = await removeProvider(p.id, { ifVersion: d.version, actor: String(v.actor ?? '') })
      // the confirm closes itself once this resolves; open the results after that
      setTimeout(() => showResults(`Removed ${p.name}`, r), 0)
      return r
    },
  })
}

// ---------------------------------------------------------------- slide-over

registerDetail('provider', {
  kind: 'Sign-in provider',
  section: 'signin',
  use: (id) => {
    const s = providersQ.use()
    const p = s.data?.providers.find((x) => x.id === id)
    if (!s.data) return { title: id, body: null, loading: !s.error, missing: s.error ? errText(s.error) : undefined }
    if (!p) return { title: <span className="mono">{id}</span>, body: null, missing: `${id} isn’t a sign-in provider (any more).` }
    const a = p.clientAuth
    return {
      title: (
        <span className="cx-cellid">
          <ProviderIcon p={p} /> {p.name}
        </span>
      ),
      chip: <Chip k="ok">enabled</Chip>,
      updated: s.at,
      foot: <Src>listSignInProviders</Src>,
      body: (
        <>
          <Strip
            items={[
              ['client auth', a.method],
              ['sign-up', p.signup],
              ['response', p.responseMode],
              ['updated', ago(Date.parse(p.updatedAt))],
            ]}
          />
          <Sec title="Provider" open>
            <KV
              rows={[
                ['ID', <span className="mono">{p.id}</span>],
                ['Issuer', <span className="mono">{p.issuer}</span>],
                ['Client ID', <span className="mono">{p.clientId}</span>],
                ['Callback URL', <Copy text={p.callbackUrl} full />],
                ['Scopes', <span className="mono">{p.scopes.length ? p.scopes.join(' ') : 'openid email'}</span>],
                ['Sign-ups per day', p.signup === 'off' ? <span className="muted">—</span> : p.signupsPerDay === 0 ? 'no cap' : String(p.signupsPerDay)],
                ['Trust email', p.trustEmail ? 'yes' : 'no'],
                ['Updated by', p.updatedBy || '—'],
              ]}
            />
          </Sec>
          <Sec title="Credentials" digest={a.method} open>
            <KV
              rows={[
                ...(usesSecret({ method: a.method, signed: !!a.signedSecret }) ? ([['Secret', <span className="mono">{storedText(a.secret)}</span>]] as [string, ReactNode][]) : []),
                ...(usesKey({ method: a.method, signed: !!a.signedSecret }) ? ([['Key', <span className="mono">{storedText(a.key)}</span>], ['Key ID', <span className="mono">{a.keyId ?? '—'}</span>]] as [string, ReactNode][]) : []),
                ...(a.signedSecret
                  ? ([
                      ['Signed by', <span className="mono">{a.signedSecret.issuer}</span>],
                      ['Audience', <span className="mono">{a.signedSecret.audience ?? '—'}</span>],
                      ['Lifetime', a.signedSecret.lifetimeSecs != null ? `${a.signedSecret.lifetimeSecs} s` : '—'],
                    ] as [string, ReactNode][])
                  : []),
              ]}
            />
          </Sec>
          <Sec title="Admit only" digest={`${Object.keys(p.requiredClaims).length} claims · ${p.idTokenAlgs.length ? p.idTokenAlgs.join(' ') : 'default algs'}`} open>
            <KV
              rows={[
                ...Object.entries(p.requiredClaims).map(([k, v]) => [<span className="mono">{k}</span>, <span className="mono">{v}</span>] as [ReactNode, ReactNode]),
                ['Algorithms', <span className="mono">{(p.idTokenAlgs.length ? p.idTokenAlgs : DEFAULT_ALGS).join(' ')}</span>],
              ]}
            />
          </Sec>
          <div className="cx-form-row">
            <button type="button" className="cx-btn sm primary" onClick={() => void editProvider(p.id)}>
              Edit…
            </button>
            <button type="button" className="cx-btn sm danger" onClick={() => void removeProviderDialog(p.id)}>
              Remove…
            </button>
          </div>
        </>
      ),
    }
  },
})

// ---------------------------------------------------------------- ⌘K

registerPalette({
  items: () => [
    {
      group: 'Actions',
      title: 'Add sign-in provider…',
      desc: 'putSignInProvider',
      run: () => {
        navigate('/admin/signin')
        void addProvider()
      },
    },
    ...(providersQ.get().data?.providers ?? []).map((p) => ({
      group: 'Sign-in providers',
      glyph: '›',
      title: p.name,
      desc: p.issuer,
      run: () => navigate(`/admin/signin?open=${encodeURIComponent(panelParam('provider', p.id))}`),
    })),
  ],
})
