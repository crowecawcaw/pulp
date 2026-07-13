import type { Workspace, Monitor, Mention, MentionPage, ChannelConfig } from '@/api/types'

export const mockWorkspace: Workspace = {
  id: 'ws-1',
  name: 'Test Workspace',
  description: null,
  created_at: 1705000000,
  updated_at: 1705000000,
}

export const mockMonitor: Monitor = {
  id: 'mon-1',
  workspace_id: 'ws-1',
  terms: ['testbrand'],
  active: true,
  channels: [],
  exact_match: false,
  case_sensitive: false,
  exclude_terms: [],
  channel_settings: {},
  ai_filter_prompt: null,
  created_at: 1705000000,
  updated_at: 1705000000,
}

export const mockMention: Mention = {
  id: 'mention-1',
  monitor_id: 'mon-1',
  channel: 'hackernews',
  external_id: 'hn-12345',
  content_text: 'testbrand is a great tool for monitoring mentions',
  content_url: 'https://news.ycombinator.com/item?id=12345',
  author_name: 'testuser',
  author_url: null,
  published_at: 1705000000,
  ingested_at: 1705001000,
  platform_meta: { points: 10 },
}

export const mockMentionWithAI: Mention = {
  ...mockMention,
  id: 'mention-2',
}

export const mockChannelConfig: ChannelConfig = {
  channel: 'hackernews',
  enabled: true,
  credentials: {},
  poll_interval: 900,
  last_polled_at: 1705000000,
  error_message: null,
  updated_at: 1705000000,
}

export const mockMentionPage: MentionPage = {
  items: [mockMention],
  has_more: false,
}
