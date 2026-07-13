// In-memory implementation of the `api` surface for demo mode.
//
// `createDemoApi()` returns an object structurally identical to the real `api`
// (the `Api` type from `@/api/client` enforces that at compile time), but every
// call reads/writes a mutable in-memory dataset seeded from `./fixtures` instead
// of hitting `/api`. Mutations (create/update/delete, mark-read, toggle channel)
// persist for the lifetime of the page so the UI feels live; a full reload
// resets to the seed data.
//
// Because the return types are checked against the generated OpenAPI types, the
// demo client cannot drift from the backend contract without a type error.

import type { Api } from '@/api/client'
import type {
  Workspace, Monitor, Mention, Notification,
  ChannelConfig, AiConfigView, MentionPage,
} from '@/api/types'
import {
  demoWorkspaces, demoMonitors, demoMentions, demoNotifications, demoChannels,
  demoAiConfig,
} from './fixtures'

let seq = 0
const uid = (prefix: string) => `${prefix}-${Date.now().toString(36)}-${seq++}`
const now = () => Math.floor(Date.now() / 1000)
const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v))
// Simulate network latency so loading states are visible in the demo.
const delay = <T>(value: T, ms = 180): Promise<T> =>
  new Promise((resolve) => setTimeout(() => resolve(clone(value)), ms))
const reject = (msg: string) => Promise.reject(new Error(msg))

export function createDemoApi(): Api {
  // Mutable session state.
  const workspaces: Workspace[] = demoWorkspaces()
  const monitors: Monitor[] = demoMonitors()
  const mentions: Mention[] = demoMentions()
  const notifications: Notification[] = demoNotifications()
  const channels: ChannelConfig[] = demoChannels()
  let ai: AiConfigView = demoAiConfig()

  return {
    workspaces: {
      list: () => delay(workspaces),
      create: (data) => {
        const ws: Workspace = {
          id: uid('ws'),
          name: data.name,
          description: data.description ?? null,
          created_at: now(),
          updated_at: now(),
        }
        workspaces.push(ws)
        return delay(ws)
      },
      update: (id, data) => {
        const ws = workspaces.find((w) => w.id === id)
        if (!ws) return reject('workspace not found')
        if (data.name !== undefined) ws.name = data.name
        if (data.description !== undefined) ws.description = data.description ?? null
        ws.updated_at = now()
        return delay(ws)
      },
      delete: (id) => {
        remove(workspaces, (w) => w.id === id)
        return delay(undefined as void)
      },
    },

    monitors: {
      list: (workspace_id) =>
        delay(monitors.filter((m) => m.workspace_id === workspace_id)),
      create: (data) => {
        const mon: Monitor = {
          id: uid('mon'),
          workspace_id: data.workspace_id,
          terms: data.terms ?? [],
          active: data.active ?? true,
          channels: data.channels ?? [],
          exact_match: data.exact_match ?? false,
          case_sensitive: data.case_sensitive ?? false,
          exclude_terms: data.exclude_terms ?? [],
          channel_settings: data.channel_settings ?? {},
          ai_filter_prompt: data.ai_filter_prompt ?? null,
          created_at: now(),
          updated_at: now(),
        }
        monitors.push(mon)
        return delay(mon)
      },
      update: (id, data) => {
        const mon = monitors.find((m) => m.id === id)
        if (!mon) return reject('monitor not found')
        Object.assign(mon, data, { updated_at: now() })
        return delay(mon)
      },
      delete: (id) => {
        remove(monitors, (m) => m.id === id)
        remove(mentions, (m) => m.monitor_id === id)
        return delay(undefined as void)
      },
    },

    mentions: {
      list: (params) => {
        let items = mentions
          .filter((m) => monitorsForWorkspace(monitors, params.workspace_id).has(m.monitor_id))
          .filter((m) => !params.channel || m.channel === params.channel)
          .filter((m) => !params.monitor_id || m.monitor_id === params.monitor_id)
          .filter((m) => matchAiFilter(m, params.ai))
          .filter((m) => (params.read === false ? m.read_at == null : true))
          .filter((m) => (params.read === true ? m.read_at != null : true))
          .filter((m) => sortKey(m) != null)
          .filter((m) => params.since === undefined || sortKey(m)! >= params.since)
          .filter((m) => params.before === undefined || sortKey(m)! < params.before)
          .sort((a, b) => sortKey(b)! - sortKey(a)!)

        const limit = params.limit ?? 50
        const has_more = items.length > limit
        items = items.slice(0, limit)
        const page: MentionPage = { items, has_more }
        return delay(page)
      },
      get: (id) => {
        const m = mentions.find((x) => x.id === id)
        return m ? delay(m) : reject('mention not found')
      },
      setRead: (id, read) => {
        const m = mentions.find((x) => x.id === id)
        if (!m) return reject('mention not found')
        m.read_at = read ? now() : null
        return delay(m)
      },
      // The demo dataset has no pending (unjudged) mentions, so the backlog is
      // always empty and the feed banner never shows.
      pendingCount: () => delay({ count: 0, oldest_ingested_at: null }),
    },

    notifications: {
      list: (workspace_id) =>
        delay(notifications.filter((n) => n.workspace_id === workspace_id)),
      create: (data) => {
        const notif: Notification = {
          id: uid('notif'),
          workspace_id: data.workspace_id,
          kind: data.kind,
          config: data.config,
          label: data.label ?? null,
          created_at: now(),
        }
        notifications.push(notif)
        return delay(notif)
      },
      delete: (id) => {
        remove(notifications, (n) => n.id === id)
        return delay(undefined as void)
      },
      test: (workspace_id) =>
        delay({ delivered: notifications.filter((n) => n.workspace_id === workspace_id).length }),
    },

    channels: {
      list: () => delay(channels),
      get: (channel) => {
        const c = channels.find((x) => x.channel === channel)
        return c ? delay(c) : reject('channel not found')
      },
      update: (channel, data) => {
        const c = channels.find((x) => x.channel === channel)
        if (!c) return reject('channel not found')
        if (data.enabled !== undefined) c.enabled = data.enabled
        if (data.credentials !== undefined) c.credentials = data.credentials
        if (data.poll_interval !== undefined) c.poll_interval = data.poll_interval
        c.updated_at = now()
        return delay(c)
      },
      cleanup: (channel, dry_run) => {
        const archived = mentions.filter(
          (m) => m.channel === channel && m.ai_verdict === 'rejected',
        )
        if (dry_run) {
          return delay({
            count: archived.length,
            sample: archived.slice(0, 5).map((m) => ({
              id: m.id,
              author: m.author_name ?? null,
              title: m.content_text.slice(0, 60),
              url: m.content_url,
            })),
          })
        }
        const ids = new Set(archived.map((m) => m.id))
        remove(mentions, (m) => ids.has(m.id))
        return delay({ deleted: archived.length })
      },
      backfill: (channel, days) =>
        delay({ message: `Backfilled ${channel} for the last ${days} days (demo: no new data fetched)` }),
      // Demo channels are on the simple poller (no durable targets).
      targets: (channel) =>
        delay({
          channel,
          targets: [],
          summary: { total: 0, healthy: 0, idle: 0, throttled: 0, failing: 0, degraded: 0, message: null },
          throttle: null,
        }),
    },

    config: {
      ai: {
        get: () => delay(ai),
        update: (data) => {
          ai = {
            enabled: data.enabled ?? ai.enabled,
            base_url: data.base_url ?? ai.base_url,
            model: data.model ?? ai.model,
            api_key_set: data.api_key != null ? data.api_key !== '' : ai.api_key_set,
          }
          return delay(ai)
        },
        test: () =>
          delay({
            ok: true,
            verdict: 'accepted',
            reason: 'Demo mode: the LLM endpoint is simulated and always accepts.',
          }),
      },
    },

    logs: {
      get: (service) =>
        delay({
          service,
          exists: true,
          lines: [
            `2026-01-01T00:00:00Z  INFO pulp::${service}: demo mode — synthetic log output`,
            `2026-01-01T00:00:05Z  INFO pulp::${service}: no live backend; these lines are fixtures`,
            `2026-01-01T00:00:10Z  INFO pulp::${service}: configure a real backend to see actual logs`,
          ],
        }),
    },

    push: {
      vapidPublicKey: () => delay({ key: 'demo-vapid-public-key' }),
    },
  }
}

// ── helpers ───────────────────────────────────────────────────────────────
function remove<T>(arr: T[], pred: (item: T) => boolean): void {
  for (let i = arr.length - 1; i >= 0; i--) if (pred(arr[i])) arr.splice(i, 1)
}

function monitorsForWorkspace(monitors: Monitor[], workspace_id: string): Set<string> {
  return new Set(monitors.filter((m) => m.workspace_id === workspace_id).map((m) => m.id))
}

// The feed orders by published_at (falling back to ingested_at), matching the
// `before`/`since` cursor FeedPage sends.
function sortKey(m: Mention): number | null {
  return m.published_at ?? m.ingested_at ?? null
}

// Mirror the backend's `ai` query param: default ("visible") shows unfiltered +
// accepted; explicit values match a specific verdict; "all" shows everything.
function matchAiFilter(m: Mention, ai: string | undefined): boolean {
  switch (ai) {
    case undefined:
    case 'visible': return m.ai_verdict == null || m.ai_verdict === 'accepted'
    case 'all': return true
    default: return m.ai_verdict === ai
  }
}
