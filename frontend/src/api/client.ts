import type { Workspace, Monitor, Notification, CreateNotification, ChannelConfig, Mention, MentionPage, PendingCount, CleanupPreview, CleanupResult, BackfillResult, AiConfigView, AiConfigUpdate, AiTestResult, LogResponse, TargetsResponse } from './types'
import { isDemoMode } from '@/demo'
import { createDemoApi } from '@/demo/client'

const BASE = '/api'

export async function apiFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    ...init,
    headers: {
      'Content-Type': 'application/json',
      ...(init?.headers ?? {}),
    },
  })
  if (!res.ok) {
    const body = await res.text()
    throw new Error(body || `HTTP ${res.status}`)
  }
  if (res.status === 204) return undefined as unknown as T
  return res.json()
}

// Typed API calls
const realApi = {
  workspaces: {
    list: () => apiFetch<Workspace[]>('/workspaces'),
    create: (data: { name: string; description?: string }) =>
      apiFetch<Workspace>('/workspaces', { method: 'POST', body: JSON.stringify(data) }),
    update: (id: string, data: { name?: string; description?: string }) =>
      apiFetch<Workspace>(`/workspaces/${id}`, { method: 'PUT', body: JSON.stringify(data) }),
    delete: (id: string) =>
      apiFetch<void>(`/workspaces/${id}`, { method: 'DELETE' }),
  },

  monitors: {
    list: (workspace_id: string) => apiFetch<Monitor[]>(`/monitors?workspace_id=${workspace_id}`),
    create: (data: Partial<Monitor> & { workspace_id: string; terms: string[] }) =>
      apiFetch<Monitor>('/monitors', { method: 'POST', body: JSON.stringify(data) }),
    update: (id: string, data: Partial<Monitor>) =>
      apiFetch<Monitor>(`/monitors/${id}`, { method: 'PUT', body: JSON.stringify(data) }),
    delete: (id: string) =>
      apiFetch<void>(`/monitors/${id}`, { method: 'DELETE' }),
  },

  mentions: {
    list: (params: { workspace_id: string; channel?: string; monitor_id?: string; limit?: number; before?: number; before_id?: string; since?: number; read?: boolean; ai?: string }) => {
      const q = new URLSearchParams()
      Object.entries(params).forEach(([k, v]) => v !== undefined && q.set(k, String(v)))
      return apiFetch<MentionPage>(`/mentions?${q}`)
    },
    get: (id: string) => apiFetch<Mention>(`/mentions/${id}`),
    setRead: (id: string, read: boolean) =>
      apiFetch<Mention>(`/mentions/${id}/read`, { method: 'PUT', body: JSON.stringify({ read }) }),
    // Size/age of the AI-filter backlog for the workspace's feed banner.
    pendingCount: (workspace_id: string) =>
      apiFetch<PendingCount>(`/mentions/pending-count?workspace_id=${workspace_id}`),
  },

  // Per-workspace notifications: dumb delivery endpoints. Everything that
  // reaches the workspace's feed fans out to all of them (no criteria).
  notifications: {
    list: (workspace_id: string) =>
      apiFetch<Notification[]>(`/notifications?workspace_id=${workspace_id}`),
    create: (data: CreateNotification) =>
      apiFetch<Notification>('/notifications', { method: 'POST', body: JSON.stringify(data) }),
    delete: (id: string) =>
      apiFetch<void>(`/notifications/${id}`, { method: 'DELETE' }),
    test: (workspace_id: string) =>
      apiFetch<{ delivered: number }>(`/notifications/test?workspace_id=${workspace_id}`, { method: 'POST' }),
  },

  channels: {
    list: () => apiFetch<ChannelConfig[]>('/channels'),
    get: (channel: string) => apiFetch<ChannelConfig>(`/channels/${channel}`),
    update: (channel: string, data: { enabled?: boolean; credentials?: Record<string, unknown>; poll_interval?: number }) =>
      apiFetch<ChannelConfig>(`/channels/${channel}`, { method: 'PUT', body: JSON.stringify(data) }),
    cleanup: (channel: string, dry_run: boolean) =>
      apiFetch<CleanupPreview | CleanupResult>(`/channels/${channel}/cleanup`, {
        method: 'POST',
        body: JSON.stringify({ dry_run }),
      }),
    backfill: (channel: string, days: number) =>
      apiFetch<BackfillResult>(`/channels/${channel}/backfill`, {
        method: 'POST',
        body: JSON.stringify({ days }),
      }),
    // Per-target collection status — the readable health view of a channel's
    // durable targets. Channels on the simple poller return an empty `targets`.
    targets: (channel: string) =>
      apiFetch<TargetsResponse>(`/channels/${channel}/targets`),
  },

  config: {
    ai: {
      get: () => apiFetch<AiConfigView>('/config/ai'),
      update: (data: AiConfigUpdate) =>
        apiFetch<AiConfigView>('/config/ai', { method: 'PUT', body: JSON.stringify(data) }),
      test: () => apiFetch<AiTestResult>('/config/ai/test', { method: 'POST' }),
    },
  },

  // Recent log output for a "service" — a channel name today, ai_filter/llm
  // later. Generic so the LogViewer component can point at any of them.
  logs: {
    get: (service: string, limit?: number) =>
      apiFetch<LogResponse>(`/logs/${service}${limit ? `?limit=${limit}` : ''}`),
  },

  push: {
    // Still needed to subscribe the browser to web push; the resulting
    // subscription is stored as a `webpush` notification (see `notifications`).
    vapidPublicKey: () => apiFetch<{ key: string }>('/push/vapid-public-key'),
  },
}

// The shape every API implementation (real or demo) must satisfy. The demo
// client (`@/demo/client`) is checked against this, so it can't drift.
export type Api = typeof realApi

// In demo mode the app talks to an in-memory dataset instead of `/api`, so it
// runs with no backend. Resolved once at module load.
export const api: Api = isDemoMode() ? createDemoApi() : realApi
