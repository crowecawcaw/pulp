import { describe, it, expect } from 'vitest'
import {
  DEMO_WORKSPACE_ID,
  DEMO_WORKSPACE_B_ID,
  demoWorkspaces,
  demoMonitors,
  demoChannels,
  demoNotifications,
  demoMentions,
  demoAiConfig,
} from '@/demo/fixtures'

// Basic assertions over the demo dataset (the data the app shows with no
// backend). These also guard the contract: e.g. monitors must use the `terms`
// model, never the removed `phrase` field.
describe('demo fixtures', () => {
  it('exposes two workspaces (a multi-project demo) with stable demo ids', () => {
    const ws = demoWorkspaces()
    expect(ws).toHaveLength(2)
    expect(ws.map((w) => w.id)).toEqual([DEMO_WORKSPACE_ID, DEMO_WORKSPACE_B_ID])
    expect(ws.map((w) => w.name)).toEqual(['Nimbus Labs', 'Fern'])
  })

  it('monitors use the match-any terms model, never a phrase', () => {
    const monitors = demoMonitors()
    expect(monitors.length).toBeGreaterThan(0)
    for (const m of monitors) {
      expect(Array.isArray(m.terms)).toBe(true)
      expect(m.terms.length).toBeGreaterThan(0)
      expect(m.terms.every((t) => typeof t === 'string' && t.trim().length > 0)).toBe(true)
      // The legacy single `phrase` field is gone.
      expect(m).not.toHaveProperty('phrase')
      // Each term is a bare literal — no OR/quote query syntax baked in.
      expect(m.terms.some((t) => / OR /i.test(t) || t.includes('"'))).toBe(false)
      expect([DEMO_WORKSPACE_ID, DEMO_WORKSPACE_B_ID]).toContain(m.workspace_id)
    }
    // Both demo workspaces own monitors.
    expect(monitors.some((m) => m.workspace_id === DEMO_WORKSPACE_ID)).toBe(true)
    expect(monitors.some((m) => m.workspace_id === DEMO_WORKSPACE_B_ID)).toBe(true)
  })

  it('covers a multi-term monitor, an excluded one, and an AI-filtered one', () => {
    const monitors = demoMonitors()
    const brand = monitors.find((m) => m.terms.includes('Nimbus'))
    expect(brand?.terms).toContain('Nimbus Labs')
    const product = monitors.find((m) => m.terms.includes('nimbusdb'))
    expect(product?.exclude_terms).toContain('hiring')
    const competitor = monitors.find((m) => m.terms.includes('Orrery'))
    expect(competitor?.ai_filter_prompt).toBeTruthy()
  })

  it('channels include enabled ones and surface a rate-limit error', () => {
    const channels = demoChannels()
    expect(channels.length).toBeGreaterThan(0)
    expect(channels.some((c) => c.enabled)).toBe(true)
    const reddit = channels.find((c) => c.channel === 'reddit')
    expect(reddit?.error_message).toMatch(/429|rate limit/i)
  })

  it('notifications are per-workspace delivery endpoints (one webpush, one webhook)', () => {
    const notifications = demoNotifications()
    expect(notifications.length).toBeGreaterThan(0)
    for (const n of notifications) {
      expect([DEMO_WORKSPACE_ID, DEMO_WORKSPACE_B_ID]).toContain(n.workspace_id)
      expect(['webpush', 'webhook']).toContain(n.kind)
      // No alert-era criteria/matching fields survive on a notification.
      expect(n).not.toHaveProperty('frequency')
      expect(n).not.toHaveProperty('criteria')
      expect(n).not.toHaveProperty('monitor_id')
    }
    const kinds = notifications.map((n) => n.kind)
    expect(kinds).toContain('webpush')
    expect(kinds).toContain('webhook')
    // The webpush config carries the browser subscription's endpoint + keys.
    const webpush = notifications.find((n) => n.kind === 'webpush')
    expect(webpush?.config.endpoint).toBeTruthy()
    // The webhook config carries its target url.
    const webhook = notifications.find((n) => n.kind === 'webhook')
    expect(webhook?.config.url).toBeTruthy()
  })

  it('mentions are well-formed (text, url, ingested after published)', () => {
    const mentions = demoMentions()
    expect(mentions.length).toBeGreaterThan(0)
    for (const m of mentions) {
      expect(m.content_text.length).toBeGreaterThan(0)
      expect(m.content_url).toMatch(/^https?:\/\//)
      expect(typeof m.monitor_id).toBe('string')
      expect(m.monitor_id.length).toBeGreaterThan(0)
      expect(typeof m.published_at).toBe('number')
      expect(typeof m.ingested_at).toBe('number')
      expect(m.ingested_at as number).toBeGreaterThanOrEqual(m.published_at as number)
    }
  })

  it('ai config points at an OpenAI-compatible base url', () => {
    const cfg = demoAiConfig()
    expect(cfg.base_url).toMatch(/\/v1$/)
    expect(typeof cfg.enabled).toBe('boolean')
  })
})
