import { useState } from 'react'
import { DataTable } from '../../components/console/DataTable'
import { Chip, ErrorState, Loading, PageHead, Panel, PanelBody, Tiles } from '../../components/console/kit'
import { ErrorNotice, Field, Spinner } from '../../components/ui'
import { chatOps } from '../../lib/chatOps'
import { useLoad } from '../../lib/hooks'

type Action = { kind: 'jobs' | 'devices'; id: string; action: 'retry' | 'revoke'; label: string; requestId: string }

export function Chat() {
  const status = useLoad(chatOps.status, [], 15000)
  const [action, setAction] = useState<Action>()
  const [reason, setReason] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<unknown>()
  const [result, setResult] = useState('')
  const choose = (next: Omit<Action, 'requestId'>) => {
    setAction({ ...next, requestId: crypto.randomUUID() }); setReason(''); setError(undefined); setResult('')
  }
  if (!status.data) return status.error ? <ErrorState error={status.error} retry={status.reload} /> : <Loading />
  const d = status.data
  const canWrite = d.role === 'operator'
  return <>
    <PageHead title="Chat" />
    <p className="sub">Signed in as {d.operator} · {d.role}. Status refreshes every 15 seconds; lists show the most recent {d.limit} entries.</p>
    <ErrorNotice error={status.error} />
    {Object.values(d.services).some(service => !service.ok) && <p role="alert">Some chat services are unavailable. Restore the service, then review pending provisioning jobs.</p>}
    <Tiles tiles={Object.entries(d.services).map(([name, service]) => ({ label: name, value: service.ok ? 'Available' : 'Unavailable', title: `Checked ${new Date(service.checkedAt).toLocaleTimeString()}`, right: <Chip k={service.ok ? 'ok' : 'err'}>{service.ok ? 'up' : 'down'}</Chip> }))} />
    <Panel title="Ingestion" right={<button className="btn" onClick={status.reload}>Refresh</button>}>
      <PanelBody><p>Source <code>{d.checkpoint.sourceId ?? 'Awaiting source'}</code></p>
      <p>Durable relay checkpoint <code>{d.checkpoint.cursor ?? 'Awaiting events'}</code></p>
      <p>Last received event: {d.ingestion?.lastReceivedAt ? new Date(d.ingestion.lastReceivedAt).toLocaleString() : 'Awaiting events after startup'} · arrival lag {d.ingestion?.lastEventLagSeconds === null || d.ingestion?.lastEventLagSeconds === undefined ? 'unknown' : `${d.ingestion.lastEventLagSeconds.toFixed(1)} s`}</p></PanelBody>
    </Panel>
    {action && <Panel title={`${action.action === 'retry' ? 'Retry provisioning' : 'Revoke device'}: ${action.label}`}>
      <PanelBody><form onSubmit={async e => {
        e.preventDefault(); setBusy(true); setError(undefined)
        try {
          const response = await chatOps.act(action.kind, action.id, action.action, reason.trim(), action.requestId)
          setResult(`Operation ${response.requestId}: ${response.result}`); setAction(undefined); status.reload()
        } catch (failure) { setError(failure); status.reload() }
        finally { setBusy(false) }
      }}>
        <p>{action.action === 'revoke' ? 'Close this device’s MAS sessions and remove it from Synapse. The device must authorize again to return.' : 'Resume the saved provisioning job. Existing rooms must pass ownership and encryption checks.'}</p>
        <Field label="Reason" hint="Operational reason only. Keep message contents and private keys out of this log.">
          <input value={reason} onChange={e => setReason(e.target.value)} minLength={5} maxLength={256} required disabled={busy} />
        </Field>
        <ErrorNotice error={error} />
        <div className="row end">
          <button className="btn" type="button" disabled={busy} onClick={() => setAction(undefined)}>Cancel</button>
          <button className="btn primary" disabled={busy || reason.trim().length < 5}>{busy && <Spinner />}Confirm {action.action}</button>
        </div>
      </form></PanelBody>
    </Panel>}
    {result && <p role="status">{result}</p>}
    <Panel title="Provisioning jobs">
      <DataTable rows={d.jobs} rowKey={j => j.id} label="Chat provisioning jobs" cols={[
        { id: 'community', label: 'Community', render: j => <code>{j.communityUri}</code> },
        { id: 'state', label: 'State', render: j => <Chip k={j.state === 'failed' ? 'err' : j.state === 'done' ? 'ok' : 'info'}>{j.state}</Chip> },
        { id: 'attempts', label: 'Attempts', r: true, render: j => j.attempts },
        { id: 'action', label: '', render: j => canWrite && ['pending', 'failed'].includes(j.state) && <button className="btn" disabled={busy} onClick={() => choose({ kind: 'jobs', id: j.id, action: 'retry', label: j.communityUri })}>Retry</button> },
      ]} />
    </Panel>
    <Panel title="Community rooms">
      <DataTable rows={d.communities} rowKey={c => c.communityUri} label="Community rooms" cols={[
        { id: 'community', label: 'Community', render: c => c.slug },
        { id: 'mode', label: 'Layout', render: c => c.chamberMode },
        { id: 'room', label: 'Space', render: c => <code>{c.spaceId}</code> },
      ]} />
    </Panel>
    <Panel title="Projection failures" foot="Role/removal failures use the existing bounded retry worker. Legacy room failures need provenance review before adoption.">
      <details><summary className="cx-pn-b">{d.failures.length} recent failures · expand for review</summary>
      <DataTable rows={d.failures} rowKey={failure => String(failure.id)} label="Projection failures" cols={[
        { id: 'event', label: 'Operation', render: failure => failure.eventType },
        { id: 'community', label: 'Community', render: failure => <code>{failure.communityUri}</code> },
        { id: 'attempts', label: 'Retries', r: true, render: failure => failure.retryCount },
      ]} /></details>
    </Panel>
    <Panel title="Attested devices">
      <div style={{ maxHeight: 360, overflow: 'auto' }}><DataTable rows={d.devices} rowKey={device => device.id} label="Attested Matrix devices" cols={[
        { id: 'device', label: 'Device', render: device => <>{device.friendlyName ?? 'Unnamed device'} <code>{device.deviceId}</code></> },
        { id: 'seen', label: 'Last seen', render: device => new Date(device.lastSeenAt).toLocaleString() },
        { id: 'state', label: 'State', render: device => <Chip k={device.revokedAt ? 'idle' : 'ok'}>{device.revokedAt ? 'Revoked' : 'Active'}</Chip> },
        { id: 'action', label: '', render: device => canWrite && <button className="btn" disabled={busy} onClick={() => choose({ kind: 'devices', id: device.id, action: 'revoke', label: device.friendlyName ?? device.deviceId })}>{device.revokedAt ? 'Enforce revocation' : 'Revoke'}</button> },
      ]} /></div>
    </Panel>
    <Panel title="Operator audit">
      <DataTable rows={d.audit} rowKey={entry => entry.requestId} label="Chat operator audit" cols={[
        { id: 'when', label: 'When', render: entry => new Date(entry.created).toLocaleString() },
        { id: 'operator', label: 'Operator', render: entry => entry.operator },
        { id: 'action', label: 'Action / target', render: entry => <>{entry.action} <code title={entry.target}>{entry.target.slice(0, 12)}</code></> },
        { id: 'reason', label: 'Reason', render: entry => entry.reason },
        { id: 'result', label: 'Result', render: entry => entry.result },
      ]} />
    </Panel>
  </>
}
