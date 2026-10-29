import { useState } from 'react'
import { CopyText, ErrorNotice, Field, Loading, Panel, Status } from '../../components/ui'
import { useAction, useLoad, useSession } from '../../lib/hooks'
import { acall, call, setSession, XrpcError } from '../../lib/xrpc'

type Factor = { authFactorType?: string; authFactorMode?: string }
type Challenge = { challenge: string; did: string; audience: string }

export function Im8() {
  const session = useSession()!
  const factor = useLoad<Factor | null>(async () => {
    try { return await acall('com.para.account.getAuthFactor') }
    catch (e) { if (e instanceof XrpcError && e.status === 501) return null; throw e }
  }, [])
  const metadata = useLoad<{ did: string }>(() => call('com.atproto.server.describeServer'), [])
  const [key, setKey] = useState('')
  const [challenge, setChallenge] = useState<Challenge>()
  const [password, setPassword] = useState('')
  const [proof, setProof] = useState('')
  const enabled = factor.data?.authFactorType === 'im8'
  const begin = useAction(async () => {
    setProof('')
    setChallenge(enabled ? await acall('vlpds.para.startIm8Removal', { body: {} }) : await acall('vlpds.para.startIm8Enrollment', { body: { publicKey: key.trim() } }))
  })
  const finish = useAction(async () => {
    await acall('com.para.account.setAuthFactor', { body: { authFactorType: enabled ? undefined : 'im8', password, authFactorToken: proof.trim() } })
    setPassword('')
    setProof('')
    setSession(null)
    location.href = '/account'
  })
  if (factor.data === null) return null
  return <Panel title="iM8 wallet authentication" desc="Approve sign-ins with your wallet independently of your M8 session. Enabling or removing this factor signs out every session." id="im8">
    <ErrorNotice error={factor.error || metadata.error || begin.error || finish.error} />
    {factor.data === undefined ? <Loading /> : <>
      <Status kind={enabled ? 'ok' : 'idle'}>{enabled ? 'On' : 'Off'}</Status>
      <p>Wallet approval is required for password, app-password and passkey sign-ins while enabled. Recover the authentication key with your backed-up wallet phrase. Email password resets keep this factor enabled.</p>
      <dl className="dl"><dt>Account</dt><dd><CopyText text={session.did} /></dd><dt>Server identity</dt><dd>{metadata.data && <CopyText text={metadata.data.did} />}</dd></dl>
      {!enabled && <Field label="Authentication public key from iM8"><input value={key} onChange={e => { setKey(e.target.value); setChallenge(undefined) }} autoComplete="off" spellCheck={false} maxLength={64} /></Field>}
      <button type="button" className="btn" disabled={begin.busy || (!enabled && key.length !== 64)} onClick={() => begin.run()}>{challenge ? 'Get fresh challenge' : enabled ? 'Start removal' : 'Start wallet enrollment'}</button>
      {challenge && <form onSubmit={e => { e.preventDefault(); finish.run() }}>
        <p>Copy this challenge into iM8, check the account and server, and approve {enabled ? 'removal' : 'enrollment'} within 90 seconds.</p>
        <CopyText text={challenge.challenge} />
        <Field label="Account password"><input type="password" value={password} onChange={e => setPassword(e.target.value)} autoComplete="current-password" required /></Field>
        <Field label="iM8 approval token"><input type="password" value={proof} onChange={e => setProof(e.target.value)} autoComplete="off" maxLength={4096} required /></Field>
        <button className="btn primary" disabled={finish.busy}>{enabled ? 'Remove wallet authentication' : 'Enable wallet authentication'}</button>
      </form>}
    </>}
  </Panel>
}
