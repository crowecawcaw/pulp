import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, waitFor, act } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { QueryClientProvider } from '@tanstack/react-query'
import FeedPage from '@/pages/FeedPage'
import { useWorkspaceStore } from '@/stores/workspace'
import { mockWorkspace, mockMention } from '@/test/mocks/api'
import { api } from '@/api/client'
import { ToastProvider } from '@/components/ui/toast'
import { createTestQueryClient } from '@/test/setup-utils'
import type { Mention } from '@/api/types'

function renderFeed() {
  return render(
    <QueryClientProvider client={createTestQueryClient()}>
      <MemoryRouter>
        <ToastProvider>
          <FeedPage />
        </ToastProvider>
      </MemoryRouter>
    </QueryClientProvider>
  )
}

vi.mock('@/api/client', () => {
  return {
    api: {
      mentions: {
        list: vi.fn(),
      },
      monitors: {
        list: vi.fn(),
      },
      channels: {
        list: vi.fn(),
      },
    },
    apiFetch: vi.fn(),
  }
})

// Mock EventSource (SSE) globally before any tests run
const mockEventSourceInstances: MockEventSource[] = []
class MockEventSource {
  static readonly CONNECTING = 0
  static readonly OPEN = 1
  static readonly CLOSED = 2
  readyState = 0
  onmessage: ((e: MessageEvent) => void) | null = null
  onerror: ((e: Event) => void) | null = null
  addEventListener = vi.fn()
  removeEventListener = vi.fn()
  close = vi.fn()
  constructor() {
    mockEventSourceInstances.push(this)
  }
}
globalThis.EventSource = MockEventSource as unknown as typeof EventSource

// A Nimbus-flavored mention on the "github" channel, used to exercise the SSE
// filter-matching test below.
const nimbusGithubMention: Mention = {
  id: 'mention-nimbus-gh',
  monitor_id: 'mon-1',
  channel: 'github',
  external_id: 'gh-9001',
  content_text: 'nimbusdb connection pool exhausts under load when max_connections is low.',
  content_url: 'https://github.com/nimbus-labs/nimbus/issues/2231',
  author_name: 'lena_okoro',
  author_url: null,
  published_at: 1705002000,
  ingested_at: 1705002000,
  platform_meta: {},
}

// The same event, but on "reddit" instead — matches a reddit-only filter.
const nimbusRedditMention: Mention = {
  ...nimbusGithubMention,
  id: 'mention-nimbus-reddit',
  channel: 'reddit',
  content_url: 'https://reddit.com/r/selfhosted/comments/nimbus',
}

describe('FeedPage', () => {
  beforeEach(() => {
    // Reset implementations each time
    vi.mocked(api.mentions.list).mockReset()
    vi.mocked(api.monitors.list).mockReset()
    vi.mocked(api.channels.list).mockReset()
    vi.mocked(api.mentions.list).mockResolvedValue({ items: [mockMention], has_more: false })
    vi.mocked(api.monitors.list).mockResolvedValue([])
    vi.mocked(api.channels.list).mockResolvedValue([])
    useWorkspaceStore.setState({ current: mockWorkspace, workspaces: [mockWorkspace] })
    mockEventSourceInstances.length = 0
  })

  it('renders the mention feed', async () => {
    renderFeed()

    await waitFor(() => {
      expect(screen.getByText(/testbrand is a great tool/i)).toBeInTheDocument()
    })
  })

  it('calls api.mentions.list with workspace_id', async () => {
    renderFeed()

    await waitFor(() => {
      expect(api.mentions.list).toHaveBeenCalledWith(
        expect.objectContaining({ workspace_id: 'ws-1' })
      )
    })
  })

  it('shows channel filter label', async () => {
    renderFeed()

    expect(screen.getByText(/^channel$/i)).toBeInTheDocument()
  })

  it('shows no unread mentions message with a read-mentions link when empty', async () => {
    vi.mocked(api.mentions.list).mockResolvedValue({ items: [], has_more: false })
    renderFeed()

    await waitFor(() => {
      expect(screen.getByText(/no unread mentions/i)).toBeInTheDocument()
    })
    expect(screen.getByRole('button', { name: /see read mentions/i })).toBeInTheDocument()
  })

  // Regression: the feed's "load more" cursor used to be a bare
  // `before = published_at ?? ingested_at`, which the backend paginated with
  // a single, non-unique, nullable `published_at < before` filter — rows that
  // tied on `published_at` (or had none) could be skipped or duplicated
  // across the page boundary. The fix is a compound keyset cursor: the
  // effective timestamp AND the last mention's `id` as a tiebreaker, both
  // sent together on every "load more" request.
  it('sends the compound before/before_id cursor when loading more', async () => {
    vi.mocked(api.mentions.list).mockResolvedValueOnce({ items: [mockMention], has_more: true })
    const user = userEvent.setup()
    renderFeed()

    await waitFor(() => {
      expect(screen.getByText(/testbrand is a great tool/i)).toBeInTheDocument()
    })

    vi.mocked(api.mentions.list).mockResolvedValueOnce({ items: [], has_more: false })
    const loadMoreBtn = await screen.findByRole('button', { name: /load more/i })
    await user.click(loadMoreBtn)

    await waitFor(() => {
      expect(api.mentions.list).toHaveBeenLastCalledWith(
        expect.objectContaining({
          before: mockMention.published_at,
          before_id: mockMention.id,
        })
      )
    })
  })

  it('drops a streamed mention that does not match the active channel filter', async () => {
    vi.mocked(api.mentions.list).mockResolvedValue({ items: [], has_more: false })
    const user = userEvent.setup()
    renderFeed()

    // Select the "GitHub" channel filter.
    const githubPill = await screen.findByRole('button', { name: /github/i })
    await user.click(githubPill)

    await waitFor(() => {
      expect(api.mentions.list).toHaveBeenCalledWith(
        expect.objectContaining({ channel: 'github' })
      )
    })

    const sse = mockEventSourceInstances[mockEventSourceInstances.length - 1]
    expect(sse).toBeDefined()

    // A streamed mention on "reddit" doesn't match the active "github" filter — it
    // must not be prepended into the visible feed.
    act(() => {
      sse.onmessage?.({ data: JSON.stringify(nimbusRedditMention) } as MessageEvent)
    })
    expect(screen.queryByText(/nimbusdb connection pool exhausts/i)).not.toBeInTheDocument()

    // The same mention on "github" does match, and should show up.
    act(() => {
      sse.onmessage?.({ data: JSON.stringify(nimbusGithubMention) } as MessageEvent)
    })
    await waitFor(() => {
      expect(screen.getByText(/nimbusdb connection pool exhausts/i)).toBeInTheDocument()
    })
  })
})
