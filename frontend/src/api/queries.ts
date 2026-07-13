// React Query hooks wrapping the existing `api` client surface.
//
// Every GET read is a `useQuery`; every write is a `useMutation` that
// invalidates the affected query keys on success. The hooks call `api.X.Y`
// lazily inside the query/mutation function (never destructured at module
// load) so the real-vs-demo selection in `@/api/client` is respected, and so
// tests that `vi.mock('@/api/client')` with a partial `api` keep working.
//
// Global query behaviour (5s polling, staleTime 0, refetchOnMount) lives in
// `./queryClient` — these hooks only declare keys and fetchers.

import {
  useQuery,
  useMutation,
  useQueryClient,
  type UseQueryOptions,
} from '@tanstack/react-query'
import { api } from './client'
import type {
  Workspace, Monitor, Notification, CreateNotification, ChannelConfig, Mention,
  MentionPage, PendingCount, CleanupPreview, CleanupResult, BackfillResult,
  AiConfigView, AiConfigUpdate, AiTestResult, LogResponse, TargetsResponse,
} from './types'

// ── Query keys ──────────────────────────────────────────────────────────────
// Centralised so query hooks and mutation invalidations can't drift apart.
export const queryKeys = {
  workspaces: ['workspaces'] as const,
  monitors: (workspaceId: string | undefined) => ['monitors', workspaceId] as const,
  mentions: (params: MentionsParams | undefined) => ['mentions', params] as const,
  mention: (id: string | undefined) => ['mention', id] as const,
  notifications: (workspaceId: string | undefined) => ['notifications', workspaceId] as const,
  pendingCount: (workspaceId: string | undefined) => ['pendingCount', workspaceId] as const,
  channels: ['channels'] as const,
  channel: (channel: string) => ['channel', channel] as const,
  channelTargets: (channel: string) => ['channelTargets', channel] as const,
  logs: (service: string, limit?: number) => ['logs', service, limit] as const,
  aiConfig: ['aiConfig'] as const,
}

export type MentionsParams = Parameters<typeof api.mentions.list>[0]

// ── Queries ───────────────────────────────────────────────────────────────

export function useWorkspaces() {
  return useQuery({
    queryKey: queryKeys.workspaces,
    queryFn: () => api.workspaces.list(),
  })
}

export function useMonitors(workspaceId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.monitors(workspaceId),
    queryFn: () => api.monitors.list(workspaceId!),
    enabled: !!workspaceId,
  })
}

export function useNotifications(workspaceId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.notifications(workspaceId),
    queryFn: () => api.notifications.list(workspaceId!),
    enabled: !!workspaceId,
  })
}

export function useMentions(
  params: MentionsParams | undefined,
  options?: Partial<UseQueryOptions<MentionPage>>,
) {
  return useQuery({
    queryKey: queryKeys.mentions(params),
    queryFn: () => api.mentions.list(params!),
    enabled: !!params,
    ...options,
  })
}

// Size/age of the AI-filter backlog for the current workspace — drives the
// feed's "N pending AI filter" banner. Polls on the global interval.
export function usePendingCount(workspaceId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.pendingCount(workspaceId),
    queryFn: () => api.mentions.pendingCount(workspaceId!),
    enabled: !!workspaceId,
  })
}

// A single mention by id — backs the detail page that web-push notifications
// deep-link to. `enabled` guards against an empty id from the route param.
export function useMention(id: string | undefined) {
  return useQuery({
    queryKey: queryKeys.mention(id),
    queryFn: () => api.mentions.get(id!),
    enabled: !!id,
  })
}

export function useChannels() {
  return useQuery({
    queryKey: queryKeys.channels,
    queryFn: () => api.channels.list(),
  })
}

export function useChannel(channel: string) {
  return useQuery({
    queryKey: queryKeys.channel(channel),
    queryFn: () => api.channels.get(channel),
    enabled: !!channel,
    retry: false, // an unconfigured channel 404s; that's an expected "no config" state
  })
}

export function useChannelTargets(channel: string) {
  return useQuery({
    queryKey: queryKeys.channelTargets(channel),
    queryFn: () => api.channels.targets(channel),
    enabled: !!channel,
  })
}

export function useLogs(service: string, limit?: number) {
  return useQuery({
    queryKey: queryKeys.logs(service, limit),
    queryFn: () => api.logs.get(service, limit),
    enabled: !!service,
  })
}

export function useAiConfig() {
  return useQuery({
    queryKey: queryKeys.aiConfig,
    queryFn: () => api.config.ai.get(),
  })
}

// ── Mutations ───────────────────────────────────────────────────────────────
// Each `onSuccess` invalidates the query keys its write affects so the UI
// refreshes from server state rather than hand-maintained local arrays.

export function useCreateWorkspace() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (data: { name: string; description?: string }) => api.workspaces.create(data),
    onSuccess: () => { qc.invalidateQueries({ queryKey: queryKeys.workspaces }) },
  })
}

export function useUpdateWorkspace() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, data }: { id: string; data: { name?: string; description?: string } }) =>
      api.workspaces.update(id, data),
    onSuccess: () => { qc.invalidateQueries({ queryKey: queryKeys.workspaces }) },
  })
}

export function useDeleteWorkspace() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => api.workspaces.delete(id),
    onSuccess: () => { qc.invalidateQueries({ queryKey: queryKeys.workspaces }) },
  })
}

type MonitorCreate = Parameters<typeof api.monitors.create>[0]

export function useCreateMonitor(workspaceId: string | undefined) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (data: MonitorCreate) => api.monitors.create(data),
    onSuccess: () => { qc.invalidateQueries({ queryKey: queryKeys.monitors(workspaceId) }) },
  })
}

export function useUpdateMonitor(workspaceId: string | undefined) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, data }: { id: string; data: Partial<Monitor> }) => api.monitors.update(id, data),
    onSuccess: () => { qc.invalidateQueries({ queryKey: queryKeys.monitors(workspaceId) }) },
  })
}

export function useDeleteMonitor(workspaceId: string | undefined) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => api.monitors.delete(id),
    onSuccess: () => { qc.invalidateQueries({ queryKey: queryKeys.monitors(workspaceId) }) },
  })
}

export function useCreateNotification(workspaceId: string | undefined) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (data: CreateNotification) => api.notifications.create(data),
    onSuccess: () => { qc.invalidateQueries({ queryKey: queryKeys.notifications(workspaceId) }) },
  })
}

export function useDeleteNotification(workspaceId: string | undefined) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => api.notifications.delete(id),
    onSuccess: () => { qc.invalidateQueries({ queryKey: queryKeys.notifications(workspaceId) }) },
  })
}

// Send a manual test to every notification in the workspace.
export function useTestNotifications(workspaceId: string | undefined) {
  return useMutation({ mutationFn: () => api.notifications.test(workspaceId!) })
}

type ChannelUpdateData = Parameters<typeof api.channels.update>[1]

export function useUpdateChannel() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ channel, data }: { channel: string; data: ChannelUpdateData }) =>
      api.channels.update(channel, data),
    onSuccess: (_res, { channel }) => {
      qc.invalidateQueries({ queryKey: queryKeys.channels })
      qc.invalidateQueries({ queryKey: queryKeys.channel(channel) })
    },
  })
}

export function useChannelCleanup() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ channel, dryRun }: { channel: string; dryRun: boolean }) =>
      api.channels.cleanup(channel, dryRun),
    onSuccess: (_res, { dryRun, channel }) => {
      // A real cleanup (not a dry-run preview) deletes mentions — refresh them.
      if (!dryRun) {
        qc.invalidateQueries({ queryKey: ['mentions'] })
        qc.invalidateQueries({ queryKey: queryKeys.channel(channel) })
      }
    },
  })
}

export function useChannelBackfill() {
  return useMutation({
    mutationFn: ({ channel, days }: { channel: string; days: number }) =>
      api.channels.backfill(channel, days),
  })
}

export function useUpdateAiConfig() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (data: AiConfigUpdate) => api.config.ai.update(data),
    onSuccess: (updated) => { qc.setQueryData(queryKeys.aiConfig, updated) },
  })
}

export function useTestAiConfig() {
  return useMutation({ mutationFn: () => api.config.ai.test() })
}

export function useSetMentionRead() {
  return useMutation({
    mutationFn: ({ id, read }: { id: string; read: boolean }) => api.mentions.setRead(id, read),
  })
}

// Re-export the value types so consumers can import everything from one place.
export type {
  Workspace, Monitor, Notification, CreateNotification, ChannelConfig, Mention,
  MentionPage, PendingCount, CleanupPreview, CleanupResult, BackfillResult,
  AiConfigView, AiConfigUpdate, AiTestResult, LogResponse, TargetsResponse,
}
