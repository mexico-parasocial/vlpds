export type ChatOpsStatus = {
  operator: string
  role: 'viewer' | 'operator'
  services: Record<string, { ok: boolean; checkedAt: string }>
  checkpoint: { sourceId: string | null; cursor: string | null }
  ingestion: { lastEventTime: string | null; lastReceivedAt: string | null; lastEventLagSeconds: number | null } | null
  jobs: { id: string; communityUri: string; state: string; attempts: number; updated: string }[]
  devices: { id: string; deviceId: string; friendlyName: string | null; createdAt: string; lastSeenAt: string; revokedAt: string | null }[]
  communities: { communityUri: string; slug: string; chamberMode: string; spaceId: string }[]
  audit: { requestId: string; operator: string; action: string; target: string; reason: string; result: string; created: string }[]
  failures: { id: number; eventType: string; communityUri: string; retryCount: number; createdAt: string }[]
  limit: number
}

async function request<T>(path: string, body?: unknown): Promise<T> {
  const response = await fetch(`/chat-ops/v1${path}`, {
    method: body ? 'POST' : 'GET', credentials: 'same-origin', redirect: 'error', signal: AbortSignal.timeout(60000),
    headers: body ? { 'content-type': 'application/json', 'x-para-chat-action': '1' } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  })
  if (!response.ok) throw new Error(response.status === 403
    ? 'Your operator sign-in does not permit this chat operation.'
    : `Chat operations unavailable (HTTP ${response.status}). Check the private bridge and operator gateway.`)
  if (!response.headers.get('content-type')?.includes('application/json')) throw new Error('The chat operations gateway is not configured for this console.')
  return response.json() as Promise<T>
}

export const chatOps = {
  status: () => request<ChatOpsStatus>('/status'),
  act: (kind: 'jobs' | 'devices', id: string, action: 'retry' | 'revoke', reason: string, requestId: string) =>
    request<{ requestId: string; result: string }>(`/${kind}/${encodeURIComponent(id)}/${action}`, { reason, requestId }),
}
