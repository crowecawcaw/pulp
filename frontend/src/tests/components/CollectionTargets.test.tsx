import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen } from '@testing-library/react'
import { QueryClientProvider } from '@tanstack/react-query'
import CollectionTargets from '@/components/CollectionTargets'
import { api } from '@/api/client'
import { createTestQueryClient } from '@/test/setup-utils'
import type { TargetStatus, TargetHealth, ChannelHealthSummary } from '@/api/types'

function renderTargets(ui: React.ReactElement) {
  return render(
    <QueryClientProvider client={createTestQueryClient()}>{ui}</QueryClientProvider>
  )
}

vi.mock('@/api/client', () => {
  return {
    api: {
      channels: {
        targets: vi.fn(),
      },
    },
    apiFetch: vi.fn(),
  }
})

const now = () => Math.floor(Date.now() / 1000)

// Health is now classified server-side; the component just renders `t.health`.
function target(over: Partial<TargetStatus> & { health?: TargetHealth }): TargetStatus {
  return {
    id: 'reddit:search:foo',
    channel: 'reddit',
    kind: 'search',
    descriptor: 'r/accessibility',
    confirmed_watermark: now() - 600,
    last_success_at: now() - 120,
    last_attempt_at: now() - 120,
    consecutive_failures: 0,
    last_error: null,
    updated_at: now(),
    health: 'healthy',
    open_jobs: [],
    ...over,
  }
}

function summary(over: Partial<ChannelHealthSummary> = {}): ChannelHealthSummary {
  return {
    total: 0, healthy: 0, idle: 0, throttled: 0, failing: 0,
    degraded: 0, message: null, ...over,
  }
}

describe('CollectionTargets rendering', () => {
  beforeEach(() => vi.mocked(api.channels.targets).mockReset())
  afterEach(() => vi.restoreAllMocks())

  it('shows the simple-poller note when there are no targets', async () => {
    vi.mocked(api.channels.targets).mockResolvedValue({ channel: 'hackernews', targets: [], summary: summary(), throttle: null })
    renderTargets(<CollectionTargets channel="hackernews" />)
    expect(await screen.findByText(/simple poller/i)).toBeInTheDocument()
  })

  it('renders the live pace when a throttle snapshot is present', async () => {
    vi.mocked(api.channels.targets).mockResolvedValue({
      channel: 'reddit',
      targets: [target({ health: 'healthy' })],
      summary: summary({ total: 1, healthy: 1 }),
      throttle: { rate_per_min: 7.5, interval_secs: 8, paused: false },
    })
    renderTargets(<CollectionTargets channel="reddit" />)
    expect(await screen.findByText(/~7\.5\/min/)).toBeInTheDocument()
    expect(screen.getByText(/1 req \/ 8s/)).toBeInTheDocument()
  })

  it('renders the server-classified health badge for a target', async () => {
    vi.mocked(api.channels.targets).mockResolvedValue({
      channel: 'reddit',
      targets: [target({ health: 'healthy' })],
      summary: summary({ total: 1, healthy: 1 }),
    })
    renderTargets(<CollectionTargets channel="reddit" />)
    expect(await screen.findAllByText('r/accessibility')).not.toHaveLength(0)
    expect(screen.getAllByText(/healthy/i).length).toBeGreaterThan(0)
    expect(api.channels.targets).toHaveBeenCalledWith('reddit')
  })

  it('renders the throttled badge for a rate-limited target', async () => {
    vi.mocked(api.channels.targets).mockResolvedValue({
      channel: 'reddit',
      targets: [target({ health: 'throttled', consecutive_failures: 1, last_error: 'HTTP 429' })],
      summary: summary({ total: 1, throttled: 1, degraded: 1, message: 'rate-limited: 1/1 targets degraded' }),
    })
    renderTargets(<CollectionTargets channel="reddit" />)
    expect(await screen.findAllByText(/throttled/i)).not.toHaveLength(0)
  })
})
