// Demo dataset for the frontend "demo mode" (see ./client.ts).
//
// This is the single source of truth for the fabricated data the app shows
// when there is no backend (a static deploy, or `?demo` in the URL). Everything
// here is typed against `@/api/types` — the SAME types generated from the Rust
// OpenAPI spec — so if the backend contract changes, regenerating types.gen.ts
// turns any drift in this file into a TypeScript compile error. That is how the
// demo data "stays in sync with the backend" without a running backend.
//
// Timestamps are computed relative to "now" at module load, so the feed always
// looks fresh and the date-range filters (last 7 / 30 days) have data to show.
//
// The two demo workspaces mirror the canonical fictional scenarios in
// AGENTS.md ("Test & demo data — canonical scenarios"): Nimbus Labs (a
// company monitoring its own self-hostable product-analytics DB) and Fern (a
// solo maintainer watching buzz about their open-source static-site
// generator). Keep this file in sync with that section and with the backend
// seed (`pulp seed`) if either changes.

import type {
  Workspace,
  Monitor,
  Mention,
  Notification,
  ChannelConfig,
  AiConfigView,
} from '@/api/types'
import { CHANNELS } from '@/api/channels'

const NOW = Math.floor(Date.now() / 1000)
const HOUR = 3600
const DAY = 86400

// Two workspaces so the demo shows how a multi-project account behaves: the
// switcher, and per-workspace monitors/feed/notifications.
export const DEMO_WORKSPACE_ID = 'demo-ws'
export const DEMO_WORKSPACE_B_ID = 'demo-ws-fern'

export function demoWorkspaces(): Workspace[] {
  return [
    {
      id: DEMO_WORKSPACE_ID,
      name: 'Nimbus Labs',
      description: 'Brand & product mentions across the developer web',
      created_at: NOW - 90 * DAY,
      updated_at: NOW - 2 * DAY,
    },
    {
      id: DEMO_WORKSPACE_B_ID,
      name: 'Fern',
      description: 'Buzz, questions, and bug reports about the Fern static-site generator',
      created_at: NOW - 45 * DAY,
      updated_at: NOW - 1 * DAY,
    },
  ]
}

// Workspace A (Nimbus Labs): brand, a specific product, and a competitor watch.
const MON_BRAND = 'demo-mon-brand'
const MON_PRODUCT = 'demo-mon-product'
const MON_COMPETITOR = 'demo-mon-competitor'
// Workspace B (Fern): project mentions + a repo-scoped build/issue watch.
const MON_FERN_PROJECT = 'demo-mon-fern-project'
const MON_FERN_BUILD = 'demo-mon-fern-build'

export function demoMonitors(): Monitor[] {
  const base = {
    workspace_id: DEMO_WORKSPACE_ID,
    active: true,
    exact_match: false,
    case_sensitive: false,
    exclude_terms: [] as string[],
    channel_settings: {},
    ai_filter_prompt: null,
    created_at: NOW - 80 * DAY,
    updated_at: NOW - 5 * DAY,
  }
  return [
    {
      ...base,
      id: MON_BRAND,
      terms: ['Nimbus', 'Nimbus Labs'],
      channels: ['hackernews', 'reddit'],
    },
    {
      ...base,
      id: MON_PRODUCT,
      terms: ['nimbusdb'],
      channels: ['github', 'hackernews'],
      exclude_terms: ['hiring', 'job'],
    },
    {
      ...base,
      id: MON_COMPETITOR,
      terms: ['Orrery'],
      channels: ['reddit', 'hackernews'],
      ai_filter_prompt: 'Only keep posts comparing Nimbus to a competitor.',
    },
    {
      ...base,
      workspace_id: DEMO_WORKSPACE_B_ID,
      id: MON_FERN_PROJECT,
      terms: ['Fern', 'fern-ssg'],
      channels: ['hackernews', 'reddit'],
    },
    {
      ...base,
      workspace_id: DEMO_WORKSPACE_B_ID,
      id: MON_FERN_BUILD,
      terms: ['fern-ssg'],
      channels: ['github', 'hackernews'],
    },
  ]
}

export function demoChannels(): ChannelConfig[] {
  const enabled = new Set<string>(CHANNELS)
  return CHANNELS.map((channel, i) => ({
    channel,
    enabled: enabled.has(channel),
    credentials: {},
    poll_interval: 900,
    last_polled_at: enabled.has(channel) ? NOW - (i + 1) * 120 : null,
    error_message:
      channel === 'reddit' ? 'reddit.com returned 429 (rate limited)' : null,
    updated_at: NOW - i * HOUR,
  }))
}

// Per-workspace notifications: dumb delivery endpoints. One web-push device and
// one webhook, mirroring the backend seed (which creates a demo webhook).
export function demoNotifications(): Notification[] {
  return [
    {
      id: 'demo-notif-1',
      workspace_id: DEMO_WORKSPACE_ID,
      kind: 'webpush',
      config: {
        endpoint: 'https://push.example.com/demo-device',
        p256dh: 'demo-p256dh',
        auth: 'demo-auth',
      },
      label: 'Demo device',
      created_at: NOW - 12 * DAY,
    },
    {
      id: 'demo-notif-2',
      workspace_id: DEMO_WORKSPACE_ID,
      kind: 'webhook',
      config: { url: 'https://hooks.example.com/services/T000/B000/demo' },
      label: 'Team chat',
      created_at: NOW - 40 * DAY,
    },
    {
      id: 'demo-notif-3',
      workspace_id: DEMO_WORKSPACE_B_ID,
      kind: 'webhook',
      config: { url: 'https://hooks.example.com/services/T000/B000/demo' },
      label: 'Team chat — deploys',
      created_at: NOW - 20 * DAY,
    },
  ]
}

export function demoAiConfig(): AiConfigView {
  return {
    enabled: false,
    base_url: 'http://localhost:11434/v1',
    model: 'llama3.2',
    api_key_set: false,
  }
}

// ── Mentions ────────────────────────────────────────────────────────────────
// A spread of realistic posts across channels and read
// state, dated over the last ~12 days so pagination and date filters have data.

type Seed = {
  monitor_id: string
  channel: Mention['channel']
  author: string
  text: string
  ageHours: number
  read?: boolean
  ai_verdict?: Mention['ai_verdict']
  meta?: Record<string, unknown>
}

const SEEDS: Seed[] = [
  {
    monitor_id: MON_BRAND, channel: 'hackernews', author: 'ops_marlowe',
    text: 'Nimbus finally shipped self-hosted mode — this is a big deal for teams that can\'t send event data to a third party.',
    ageHours: 3,
    meta: { points: 142, comments: 38 },
  },
  {
    monitor_id: MON_BRAND, channel: 'reddit', author: 'u/dataeng_dan',
    text: 'Anyone using Nimbus in production? Considering it over the usual alternatives for our event pipeline.',
    ageHours: 9,
    meta: { subreddit: 'dataengineering', upvotes: 56 },
  },
  {
    monitor_id: MON_BRAND, channel: 'reddit', author: 'u/k_hightower',
    text: 'The Nimbus SDK ergonomics are genuinely nice. One import, typed events, done.',
    ageHours: 20,
    meta: { subreddit: 'programming', upvotes: 88 },
  },
  {
    monitor_id: MON_PRODUCT, channel: 'github', author: 'kade_rios',
    text: 'nimbusdb connection pool exhausts under load when max_connections is low — repro in the linked gist.',
    ageHours: 28,
    meta: { repo: 'nimbus-labs/nimbus', issue: 2231 },
  },
  {
    monitor_id: MON_PRODUCT, channel: 'github', author: 'jdoe',
    text: 'How do I configure nimbusdb read replicas with the Rust client? The docs only cover the primary.',
    ageHours: 34,
    meta: { repo: 'nimbus-labs/nimbus', issue: 2240 },
  },
  {
    monitor_id: MON_BRAND, channel: 'hackernews', author: 'rachelcodes',
    text: 'I migrated our dashboards from a homegrown setup to Nimbus in a weekend. Write-up inside.',
    ageHours: 40,
    read: true, meta: { points: 76, comments: 18 },
  },
  {
    monitor_id: MON_PRODUCT, channel: 'hackernews', author: 'wren_kessler',
    text: 'nimbusdb\'s WAL implementation is worth reading if you care about durability tradeoffs.',
    ageHours: 50,
    meta: { points: 61, comments: 12 },
  },
  {
    monitor_id: MON_BRAND, channel: 'reddit', author: 'u/infra_ian',
    text: 'Nimbus pricing page is confusing — is the event cap per month or per day, and does self-hosting even need a license?',
    ageHours: 62,
    meta: { subreddit: 'devops', upvotes: 12 },
  },
  {
    monitor_id: MON_COMPETITOR, channel: 'reddit', author: 'u/startup_cto',
    text: 'We evaluated Orrery vs Nimbus. Orrery was cheaper but the API was painful. Ended up on Nimbus.',
    ageHours: 74,
    ai_verdict: 'accepted',
    meta: { subreddit: 'startups', upvotes: 203 },
  },
  {
    monitor_id: MON_COMPETITOR, channel: 'hackernews', author: 'hiring_roundup',
    text: 'Orrery is hiring senior engineers — DM me.',
    ageHours: 80,
    ai_verdict: 'rejected', meta: { points: 2 },
  },
  {
    monitor_id: MON_PRODUCT, channel: 'hackernews', author: 'thedevreview',
    text: 'nimbusdb in 100 seconds — a quick tour of the query planner and the new vector index.',
    ageHours: 96,
    read: true, meta: { points: 54, comments: 9 },
  },
  {
    monitor_id: MON_BRAND, channel: 'reddit', author: 'u/grumpy_sre',
    text: 'Nimbus had an outage this morning and the status page was a lie. Not impressed.',
    ageHours: 110,
    meta: { subreddit: 'devops', upvotes: 77 },
  },
  {
    monitor_id: MON_BRAND, channel: 'hackernews', author: 'maria_dev',
    text: 'Five things I wish I knew before adopting Nimbus.',
    ageHours: 130,
    read: true, meta: { points: 39, comments: 14 },
  },
  {
    monitor_id: MON_COMPETITOR, channel: 'reddit', author: 'u/makerleah',
    text: 'Orrery just launched v3. Curious how it stacks up against Nimbus now.',
    ageHours: 150,
    ai_verdict: 'accepted', meta: { subreddit: 'SaaS', upvotes: 140 },
  },
  {
    monitor_id: MON_PRODUCT, channel: 'github', author: 'contributor99',
    text: 'PR: add connection retry with backoff to the nimbusdb Go driver.',
    ageHours: 175,
    read: true, meta: { repo: 'nimbus-labs/nimbus-go', pr: 88 },
  },
  {
    monitor_id: MON_BRAND, channel: 'hackernews', author: 'dataquill',
    text: 'Nimbus export API is great — I piped a year of events into DuckDB in minutes.',
    ageHours: 200,
    read: true, meta: { points: 98, comments: 21 },
  },
  {
    monitor_id: MON_PRODUCT, channel: 'github', author: 'devops_amy',
    text: 'nimbusdb Helm chart fails on k8s 1.30 — readiness probe path changed?',
    ageHours: 220,
    meta: { repo: 'nimbus-labs/nimbus', issue: 2255 },
  },
  {
    monitor_id: MON_BRAND, channel: 'reddit', author: 'u/nimbus_founder',
    text: 'Thank you to everyone who tried Nimbus Labs this week. 2,000 new workspaces!',
    ageHours: 245,
    read: true, meta: { subreddit: 'analytics', upvotes: 64 },
  },
  {
    monitor_id: MON_PRODUCT, channel: 'hackernews', author: 'perf_okonkwo',
    text: 'Benchmarked nimbusdb vs the obvious alternatives. Numbers and methodology inside.',
    ageHours: 265,
    read: true, meta: { points: 210, comments: 64 },
  },
  {
    monitor_id: MON_BRAND, channel: 'reddit', author: 'u/pm_curious',
    text: 'Does Nimbus support GDPR data residency in the EU yet?',
    ageHours: 285,
    read: true, meta: { subreddit: 'gdpr', upvotes: 15 },
  },

  // ── Workspace B: Fern ───────────────────────────────────────────────────────
  {
    monitor_id: MON_FERN_PROJECT, channel: 'hackernews', author: 'iris_fern',
    text: 'Show HN: Fern, a static-site generator that builds in milliseconds and ships zero client-side JS by default.',
    ageHours: 4,
    meta: { points: 117, comments: 29 },
  },
  {
    monitor_id: MON_FERN_PROJECT, channel: 'reddit', author: 'u/frontend_otto',
    text: 'Rebuilt my blog with Fern (fern-ssg) this weekend — the incremental build speed alone was worth the migration.',
    ageHours: 15,
    meta: { subreddit: 'webdev', upvotes: 41 },
  },
  {
    monitor_id: MON_FERN_BUILD, channel: 'github', author: 'lena_okoro',
    text: 'fern-ssg build fails with "unexpected token" on nested MDX frontmatter — repro repo attached.',
    ageHours: 26,
    meta: { repo: 'fern-ssg/fern', issue: 312 },
  },
  {
    monitor_id: MON_FERN_BUILD, channel: 'github', author: 'ci_botley',
    text: 'How do I write a custom fern-ssg plugin for image optimization? The docs only cover the built-in ones.',
    ageHours: 48,
    meta: { repo: 'fern-ssg/fern', issue: 318 },
  },
  {
    monitor_id: MON_FERN_PROJECT, channel: 'hackernews', author: 'deploywatch',
    text: 'Fern vs Bramble for a docs site — we picked Fern for the simpler config and faster cold builds.',
    ageHours: 90,
    read: true, meta: { points: 88, comments: 17 },
  },
  {
    monitor_id: MON_FERN_BUILD, channel: 'github', author: 'contributor42',
    text: 'Feature request: fern-ssg should support draft posts that build locally but are excluded from `fern build --prod`.',
    ageHours: 130,
    read: true, meta: { repo: 'fern-ssg/fern', issue: 325 },
  },
  {
    monitor_id: MON_FERN_PROJECT, channel: 'reddit', author: 'u/k8s_pim',
    text: 'Showcase: migrated a 400-page docs site to Fern, cut CI build time from 6 minutes to 40 seconds.',
    ageHours: 190,
    read: true, meta: { subreddit: 'javascript', upvotes: 96 },
  },
]

export function demoMentions(): Mention[] {
  return SEEDS.map((s, i) => {
    const published = NOW - s.ageHours * HOUR
    return {
      id: `demo-mention-${i + 1}`,
      monitor_id: s.monitor_id,
      channel: s.channel,
      external_id: `${s.channel}-${1000 + i}`,
      content_text: s.text,
      content_url: contentUrl(s.channel, 1000 + i, s.meta),
      author_name: s.author,
      author_url: null,
      published_at: published,
      ingested_at: published + 300,
      platform_meta: s.meta ?? {},
      read_at: s.read ? published + 600 : null,
      ai_verdict: s.ai_verdict ?? null,
      ai_reason: s.ai_verdict
        ? s.ai_verdict === 'accepted'
          ? 'Relevant comparison of the product against a competitor'
          : 'Recruiting post, not a product discussion'
        : null,
    }
  })
}

function contentUrl(channel: string, id: number, meta?: Record<string, unknown>): string {
  switch (channel) {
    case 'hackernews': return `https://news.ycombinator.com/item?id=${id}`
    case 'reddit': return `https://reddit.com/r/_/comments/${id}`
    case 'github': {
      const repo = typeof meta?.repo === 'string' ? meta.repo : 'nimbus-labs/nimbus'
      return `https://github.com/${repo}/issues/${id}`
    }
    default: return `https://example.com/${id}`
  }
}
