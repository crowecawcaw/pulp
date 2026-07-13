// API domain types.
//
// The Rust backend is the single source of truth for the API contract. The file
// `types.gen.ts` is generated from the served OpenAPI spec
// (`npm run gen:api` against a running backend, or `npm run gen:api:file`
// against the checked-in `../backend/openapi.json`). Do NOT edit `types.gen.ts`.
//
// This module re-exports the generated component schemas as the names the app
// uses. Where the generated type is a faithful representation of the wire shape
// we alias it directly. A handful of fields the generator can only express as
// `unknown` (free-form JSON blobs: `platform_meta`, `config`, `credentials`)
// or as plain `string` (the `kind` enum) are narrowed here so the UI keeps
// its stricter types. Those narrowings are the ONLY hand-maintained part of this
// file.

import type { components } from './types.gen'

type Schemas = components['schemas']

// ── Direct aliases (generated shape used as-is) ─────────────────────────────
export type Workspace = Schemas['Workspace']

// `channel_settings` is free-form per-channel JSON keyed by channel name; the
// UI reads/writes known keys (e.g. reddit subreddits) so keep it a record.
export type Monitor = Omit<Schemas['Monitor'], 'channel_settings'> & {
  channel_settings: Record<string, Record<string, unknown>>
}
// A monitor matches on ANY of its `terms`. For compact single-line displays
// (feed pills, select options) join them into one readable label.
export const monitorLabel = (m: Pick<Monitor, 'terms'>): string => m.terms.join(', ')

export type MentionSample = Schemas['MentionSample']
export type CleanupPreview = Schemas['CleanupPreview']
export type CleanupResult = Schemas['CleanupResult']
export type BackfillResult = Schemas['BackfillResult']

// ── Narrowed aliases (override free-form JSON / enum fields) ────────────────

// `platform_meta` is free-form JSON; the UI indexes into it, so keep it a record.
export type Mention = Omit<
  Schemas['Mention'],
  'platform_meta' | 'ai_verdict'
> & {
  platform_meta: Record<string, unknown>
  ai_verdict?: 'pending' | 'accepted' | 'rejected' | null
}

// A notification is a per-workspace delivery endpoint. `kind` selects the
// transport and `config` is the kind-specific free-form JSON the UI reads from
// (webpush: `{endpoint,p256dh,auth}`; webhook: `{url}`).
export type Notification = Omit<Schemas['Notification'], 'kind' | 'config'> & {
  kind: 'webpush' | 'webhook'
  config: Record<string, unknown>
}

// Create request body for `POST /api/notifications`.
export type CreateNotification = Omit<Schemas['CreateNotification'], 'kind' | 'config'> & {
  kind: 'webpush' | 'webhook'
  config: Record<string, unknown>
}

// `credentials` is free-form per-channel JSON the UI indexes into.
// `caught_up_at` / `max_backfill_days` are returned by the API but the UI
// does not consume them; they're kept optional so existing call sites and test
// fixtures (which predate these columns) still satisfy the type.
export type ChannelConfig = Omit<
  Schemas['ChannelConfig'],
  'credentials' | 'caught_up_at' | 'max_backfill_days'
> & {
  credentials: Record<string, unknown>
  caught_up_at?: number | null
  max_backfill_days?: number
}

// `items` is narrowed to the narrowed `Mention` above.
export type MentionPage = Omit<Schemas['MentionPage'], 'items'> & {
  items: Mention[]
}

// Size + age of the AI-filter backlog (drives the feed's "N pending" banner).
export type PendingCount = Schemas['PendingCount']

// AI relevance-filter settings (bring-your-own LLM endpoint).
export type AiConfigView = Schemas['AiConfigView']
export type AiConfigUpdate = Schemas['AiConfigUpdate']
export type AiTestResult = Schemas['AiTestResult']

// Per-service log tail (channels today, ai_filter/llm later).
export type LogResponse = Schemas['LogResponse']

// Per-target collection status (the readable view of a channel's collector
// health). `BackfillJob` is the open-jobs element; `TargetStatus` extends a
// `CollectorTarget` with its open jobs; `TargetsResponse` wraps the list.
export type BackfillJob = Schemas['BackfillJob']
export type TargetStatus = Schemas['TargetStatus']
// `TargetsResponse.throttle` is OPTIONAL/NULLABLE: the live rate-limiter lane
// snapshot, present only after a pass materializes the lane (null for
// simple-poller channels and before the first pass).
export type TargetsResponse = Schemas['TargetsResponse']
export type ThrottleState = Schemas['ThrottleState']
// Coarse per-target health + the channel-level summary, both derived server-side
// from one classifier so the banner and the per-target table can't disagree.
export type TargetHealth = Schemas['TargetHealth']
export type ChannelHealthSummary = Schemas['ChannelHealthSummary']
