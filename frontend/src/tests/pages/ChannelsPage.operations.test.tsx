import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { QueryClientProvider } from '@tanstack/react-query'
import ChannelsPage from '@/pages/ChannelsPage'
import { api } from '@/api/client'
import { ToastProvider } from '@/components/ui/toast'
import { createTestQueryClient } from '@/test/setup-utils'
import type { ChannelConfig } from '@/api/types'

vi.mock('@/api/client', () => {
  return {
    api: {
      channels: {
        list: vi.fn(),
        update: vi.fn(),
        cleanup: vi.fn(),
        backfill: vi.fn(),
      },
    },
    apiFetch: vi.fn(),
  }
})

const mockGithubChannel: ChannelConfig = {
  channel: 'github',
  enabled: true,
  credentials: { token: 'gh_test', ignore_repos: ['nimbus-labs/*'], state_filter: 'open' },
  poll_interval: 900,
  last_polled_at: null,
  error_message: null,
  updated_at: Date.now(),
}

const mockRedditChannel: ChannelConfig = {
  channel: 'reddit',
  enabled: true,
  credentials: { user_agent: 'nimbus:v0.1 (by /u/nimbuslabs)', subreddits: ['selfhosted', 'dataengineering'] },
  poll_interval: 300,
  last_polled_at: null,
  error_message: null,
  updated_at: Date.now(),
}

function renderPage() {
  return render(
    <QueryClientProvider client={createTestQueryClient()}>
      <MemoryRouter>
        <ToastProvider>
          <ChannelsPage />
        </ToastProvider>
      </MemoryRouter>
    </QueryClientProvider>
  )
}

// Open the GitHub config dialog by clicking the gear for GitHub (index 1 among channels-with-fields in mobile view)
async function openGithubConfig(user: ReturnType<typeof userEvent.setup>) {
  await waitFor(() => {
    expect(screen.getAllByText('GitHub').length).toBeGreaterThan(0)
  })
  const gearButtons = screen.getAllByRole('button').filter((btn) => btn.querySelector('svg.lucide-settings'))
  // only reddit and github have credential fields → mobile order: reddit=0, github=1
  await user.click(gearButtons[1])
}

// Open the Reddit config dialog (index 0 among channels-with-fields in mobile view)
async function openRedditConfig(user: ReturnType<typeof userEvent.setup>) {
  await waitFor(() => {
    expect(screen.getAllByText('Reddit').length).toBeGreaterThan(0)
  })
  const gearButtons = screen.getAllByRole('button').filter((btn) => btn.querySelector('svg.lucide-settings'))
  await user.click(gearButtons[0])
}

describe('ChannelsPage – backfill and cleanup operations', () => {
  beforeEach(() => {
    vi.mocked(api.channels.list).mockReset()
    vi.mocked(api.channels.update).mockReset()
    vi.mocked(api.channels.cleanup).mockReset()
    vi.mocked(api.channels.backfill).mockReset()
    vi.mocked(api.channels.list).mockResolvedValue([mockGithubChannel, mockRedditChannel])
    vi.mocked(api.channels.update).mockResolvedValue(mockGithubChannel)
    vi.mocked(api.channels.backfill).mockResolvedValue({ message: 'Backfill complete' })
    vi.mocked(api.channels.cleanup).mockResolvedValue({ count: 0, sample: [] })
  })

  // 1. Backfill section renders in the dialog for any channel
  it('renders the backfill section when the config dialog is open for any channel', async () => {
    const user = userEvent.setup()
    renderPage()

    await openGithubConfig(user)

    await waitFor(() => {
      expect(screen.getByText('Backfill history')).toBeInTheDocument()
      expect(screen.getByRole('button', { name: /start backfill/i })).toBeInTheDocument()
    })
  })

  it('renders the backfill section for a non-github channel (Reddit)', async () => {
    const user = userEvent.setup()
    renderPage()

    await openRedditConfig(user)

    await waitFor(() => {
      expect(screen.getByText('Backfill history')).toBeInTheDocument()
      expect(screen.getByRole('button', { name: /start backfill/i })).toBeInTheDocument()
    })
  })

  // 2. Clicking "Start backfill" calls api.channels.backfill with correct channel and days
  it('calls api.channels.backfill with the correct channel and days when "Start backfill" is clicked', async () => {
    const user = userEvent.setup()
    renderPage()

    await openGithubConfig(user)

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /start backfill/i })).toBeInTheDocument()
    })

    // Default days is 30; click the button
    const backfillBtn = screen.getByRole('button', { name: /start backfill/i })
    await user.click(backfillBtn)

    await waitFor(() => {
      expect(api.channels.backfill).toHaveBeenCalledWith('github', 30)
    })
  })

  it('calls api.channels.backfill with updated days when user changes the input', async () => {
    const user = userEvent.setup()
    renderPage()

    await openGithubConfig(user)

    await waitFor(() => {
      expect(screen.getByRole('spinbutton')).toBeInTheDocument()
    })

    const daysInput = screen.getByRole('spinbutton')
    await user.clear(daysInput)
    await user.type(daysInput, '7')

    const backfillBtn = screen.getByRole('button', { name: /start backfill/i })
    await user.click(backfillBtn)

    await waitFor(() => {
      expect(api.channels.backfill).toHaveBeenCalledWith('github', 7)
    })
  })

  // 3. Cleanup section only renders when configChannel.channel === 'github'
  it('renders the cleanup section only for the github channel', async () => {
    const user = userEvent.setup()
    renderPage()

    await openGithubConfig(user)

    await waitFor(() => {
      expect(screen.getByText('Remove old noise')).toBeInTheDocument()
      expect(screen.getByRole('button', { name: /check for noise/i })).toBeInTheDocument()
    })
  })

  it('does NOT render the cleanup section for a non-github channel (Reddit)', async () => {
    const user = userEvent.setup()
    renderPage()

    await openRedditConfig(user)

    await waitFor(() => {
      expect(screen.getByText('Backfill history')).toBeInTheDocument()
    })

    expect(screen.queryByText('Remove old noise')).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /check for noise/i })).not.toBeInTheDocument()
  })

  // 4. Clicking "Check for noise" calls api.channels.cleanup with dry_run=true
  it('calls api.channels.cleanup with dry_run=true when "Check for noise" is clicked', async () => {
    vi.mocked(api.channels.cleanup).mockResolvedValue({ count: 0, sample: [] })

    const user = userEvent.setup()
    renderPage()

    await openGithubConfig(user)

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /check for noise/i })).toBeInTheDocument()
    })

    await user.click(screen.getByRole('button', { name: /check for noise/i }))

    await waitFor(() => {
      expect(api.channels.cleanup).toHaveBeenCalledWith('github', true)
    })
  })

  // 5. When preview shows count > 0, the "Delete N mentions" button appears
  it('shows the "Delete N mentions" button when cleanup preview has count > 0', async () => {
    vi.mocked(api.channels.cleanup).mockResolvedValue({
      count: 3,
      sample: [
        { id: 's1', repo: 'org/repo', author: 'user1', title: 'Issue one', url: 'https://github.com/org/repo/issues/1' },
        { id: 's2', repo: 'org/repo', author: 'user2', title: 'Issue two', url: 'https://github.com/org/repo/issues/2' },
      ],
    })

    const user = userEvent.setup()
    renderPage()

    await openGithubConfig(user)

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /check for noise/i })).toBeInTheDocument()
    })

    await user.click(screen.getByRole('button', { name: /check for noise/i }))

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /delete 3 mentions/i })).toBeInTheDocument()
    })

    // Sample items should also be visible — text is split across elements so use a substring search
    expect(screen.getByText(/Issue one/)).toBeInTheDocument()
  })

  // 6. Clicking "Delete N mentions" calls api.channels.cleanup with dry_run=false
  it('calls api.channels.cleanup with dry_run=false when "Delete N mentions" is clicked', async () => {
    // First call (dry_run=true) returns a preview
    vi.mocked(api.channels.cleanup)
      .mockResolvedValueOnce({ count: 5, sample: [] })
      // Second call (dry_run=false) returns the deletion result
      .mockResolvedValueOnce({ deleted: 5 })

    const user = userEvent.setup()
    renderPage()

    await openGithubConfig(user)

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /check for noise/i })).toBeInTheDocument()
    })

    // Trigger the dry-run preview
    await user.click(screen.getByRole('button', { name: /check for noise/i }))

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /delete 5 mentions/i })).toBeInTheDocument()
    })

    // Click the destructive delete button
    await user.click(screen.getByRole('button', { name: /delete 5 mentions/i }))

    await waitFor(() => {
      expect(api.channels.cleanup).toHaveBeenCalledWith('github', false)
    })
  })
})
