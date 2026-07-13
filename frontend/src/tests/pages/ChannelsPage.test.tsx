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
      },
    },
    apiFetch: vi.fn(),
  }
})

const mockGithubChannel: ChannelConfig = {
  channel: 'github',
  enabled: true,
  credentials: { token: 'gh_test', ignore_repos: ['nimbus-labs/*', 'myorg/repo'], state_filter: 'open' },
  poll_interval: 900,
  last_polled_at: null,
  error_message: null,
  updated_at: Date.now(),
}

const mockRedditChannel: ChannelConfig = {
  channel: 'reddit',
  enabled: true,
  credentials: {
    user_agent: 'nimbus:v0.1 (by /u/nimbuslabs)',
    subreddits: ['selfhosted', 'dataengineering'],
    exclude_subreddits: ['deals'],
    exclude_authors: ['spammer'],
  },
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

describe('ChannelsPage', () => {
  beforeEach(() => {
    vi.mocked(api.channels.list).mockReset()
    vi.mocked(api.channels.update).mockReset()
    vi.mocked(api.channels.list).mockResolvedValue([mockGithubChannel, mockRedditChannel])
    vi.mocked(api.channels.update).mockResolvedValue(mockGithubChannel)
  })

  // Helper: open GitHub config dialog by finding and clicking a gear button in the GitHub row
  async function openGithubConfig(user: ReturnType<typeof userEvent.setup>) {
    // Wait for GitHub text to appear
    await waitFor(() => {
      expect(screen.getAllByText('GitHub').length).toBeGreaterThan(0)
    })

    // Only channels with credential fields render a gear (settings) button:
    // hackernews has none, so reddit and github are the only two. In ALL_CHANNELS
    // order the mobile gear buttons are reddit=0, github=1 (desktop doubles them).
    const gearButtons = screen.getAllByRole('button').filter((btn) => btn.querySelector('svg.lucide-settings'))
    expect(gearButtons.length).toBeGreaterThan(0)
    // Click the mobile GitHub gear button (index 1, after reddit's).
    await user.click(gearButtons[1])
  }

  // Helper: open Reddit config dialog (mobile gear index 0, before GitHub's).
  async function openRedditConfig(user: ReturnType<typeof userEvent.setup>) {
    await waitFor(() => {
      expect(screen.getAllByText('Reddit').length).toBeGreaterThan(0)
    })
    const gearButtons = screen.getAllByRole('button').filter((btn) => btn.querySelector('svg.lucide-settings'))
    expect(gearButtons.length).toBeGreaterThan(0)
    await user.click(gearButtons[0])
  }

  it('renders GitHub channel in the list', async () => {
    renderPage()

    await waitFor(() => {
      expect(screen.getAllByText('GitHub').length).toBeGreaterThan(0)
    })
  })

  it('opens GitHub config dialog with all expected fields', async () => {
    const user = userEvent.setup()
    renderPage()

    await openGithubConfig(user)

    await waitFor(() => {
      expect(screen.getByText('Personal Access Token')).toBeInTheDocument()
      expect(screen.getByText('Ignore repos (glob patterns, one per line)')).toBeInTheDocument()
      expect(screen.getByText('Ignore orgs (one per line)')).toBeInTheDocument()
      expect(screen.getByText('Ignore authors (one per line)')).toBeInTheDocument()
      expect(screen.getByText('Only watch these repos (glob patterns, leave blank for all)')).toBeInTheDocument()
      expect(screen.getByText('Issue state filter')).toBeInTheDocument()
    })
  })

  it('renders pattern fields (ignore_repos) as textareas', async () => {
    const user = userEvent.setup()
    renderPage()

    await openGithubConfig(user)

    await waitFor(() => {
      screen.getByText('Ignore repos (glob patterns, one per line)')
    })

    // There should be multiple textareas (ignore_repos, ignore_orgs, ignore_authors, only_repos)
    const textareas = screen.getAllByRole('textbox')
    // At least one textarea should have placeholder for ignore_repos
    const ignoreReposTextarea = textareas.find(
      (el) => (el as HTMLTextAreaElement).placeholder === 'nimbus-labs/*\nmyorg/specific-repo'
    )
    expect(ignoreReposTextarea).toBeDefined()
    expect(ignoreReposTextarea!.tagName.toLowerCase()).toBe('textarea')
  })

  it('renders state_filter as a select with correct options', async () => {
    const user = userEvent.setup()
    renderPage()

    await openGithubConfig(user)

    await waitFor(() => {
      screen.getByText('Issue state filter')
    })

    const selectEl = screen.getByRole('combobox')
    expect(selectEl.tagName.toLowerCase()).toBe('select')
    expect(screen.getByRole('option', { name: 'Open only (recommended)' })).toBeInTheDocument()
    expect(screen.getByRole('option', { name: 'All states' })).toBeInTheDocument()
    expect(screen.getByRole('option', { name: 'Closed only' })).toBeInTheDocument()
  })

  it('joins array credentials with newlines when opening config', async () => {
    const user = userEvent.setup()
    renderPage()

    await openGithubConfig(user)

    await waitFor(() => {
      screen.getByText('Ignore repos (glob patterns, one per line)')
    })

    // ignore_repos: ['nimbus-labs/*', 'myorg/repo'] should be joined as 'nimbus-labs/*\nmyorg/repo'
    const textareas = screen.getAllByRole('textbox')
    const ignoreReposTextarea = textareas.find(
      (el) => (el as HTMLTextAreaElement).placeholder === 'nimbus-labs/*\nmyorg/specific-repo'
    ) as HTMLTextAreaElement
    expect(ignoreReposTextarea).toBeDefined()
    expect(ignoreReposTextarea.value).toBe('nimbus-labs/*\nmyorg/repo')
  })

  it('serializes pattern fields as arrays when saving', async () => {
    const user = userEvent.setup()
    renderPage()

    await openGithubConfig(user)

    await waitFor(() => {
      screen.getByText('Ignore repos (glob patterns, one per line)')
    })

    // Find the ignore_repos textarea and update it
    const textareas = screen.getAllByRole('textbox')
    const ignoreReposTextarea = textareas.find(
      (el) => (el as HTMLTextAreaElement).placeholder === 'nimbus-labs/*\nmyorg/specific-repo'
    ) as HTMLTextAreaElement
    await user.clear(ignoreReposTextarea)
    await user.type(ignoreReposTextarea, 'org1/*\norg2/repo')

    // Click Save
    const saveButton = screen.getByRole('button', { name: /save/i })
    await user.click(saveButton)

    await waitFor(() => {
      expect(api.channels.update).toHaveBeenCalledWith(
        'github',
        expect.objectContaining({
          credentials: expect.objectContaining({
            ignore_repos: ['org1/*', 'org2/repo'],
          }),
        })
      )
    })
  })

  it('opens Reddit config dialog with the real RSS credential fields, not OAuth', async () => {
    const user = userEvent.setup()
    renderPage()

    await openRedditConfig(user)

    await waitFor(() => {
      expect(screen.getByText('User Agent')).toBeInTheDocument()
      expect(screen.getByText('Subreddits (one per line, leave blank to search all of Reddit)')).toBeInTheDocument()
      expect(screen.getByText('Exclude subreddits (one per line)')).toBeInTheDocument()
      expect(screen.getByText('Exclude authors (one per line)')).toBeInTheDocument()
    })

    // The RSS collector never reads OAuth credentials, so the dialog must not offer them.
    expect(screen.queryByText('Client ID')).not.toBeInTheDocument()
    expect(screen.queryByText('Client Secret')).not.toBeInTheDocument()
  })

  it('shows Nimbus-flavored placeholders for the Reddit fields', async () => {
    const user = userEvent.setup()
    renderPage()

    await openRedditConfig(user)

    await waitFor(() => {
      const textboxes = screen.getAllByRole('textbox')
      const userAgentInput = textboxes.find((el) => (el as HTMLInputElement).placeholder === 'nimbus:v0.1 (by /u/nimbuslabs)')
      const subredditsTextarea = textboxes.find((el) => (el as HTMLTextAreaElement).placeholder === 'selfhosted\ndataengineering')
      expect(userAgentInput).toBeDefined()
      expect(subredditsTextarea).toBeDefined()
    })
  })

  it('joins subreddits array credentials with newlines when opening Reddit config', async () => {
    const user = userEvent.setup()
    renderPage()

    await openRedditConfig(user)

    await waitFor(() => {
      const textboxes = screen.getAllByRole('textbox')
      const subredditsTextarea = textboxes.find(
        (el) => (el as HTMLTextAreaElement).placeholder === 'selfhosted\ndataengineering'
      ) as HTMLTextAreaElement
      expect(subredditsTextarea).toBeDefined()
      expect(subredditsTextarea.value).toBe('selfhosted\ndataengineering')
    })
  })

  it('serializes Reddit pattern fields as arrays when saving', async () => {
    const user = userEvent.setup()
    renderPage()

    await openRedditConfig(user)

    await waitFor(() => {
      screen.getByText('User Agent')
    })

    const textboxes = screen.getAllByRole('textbox')
    const subredditsTextarea = textboxes.find(
      (el) => (el as HTMLTextAreaElement).placeholder === 'selfhosted\ndataengineering'
    ) as HTMLTextAreaElement
    const userAgentInput = textboxes.find(
      (el) => (el as HTMLInputElement).placeholder === 'nimbus:v0.1 (by /u/nimbuslabs)'
    ) as HTMLInputElement

    await user.clear(subredditsTextarea)
    await user.type(subredditsTextarea, 'fern-ssg\nstaticsite')
    await user.clear(userAgentInput)
    await user.type(userAgentInput, 'fern:v0.1 (by /u/fernbot)')

    await user.click(screen.getByRole('button', { name: /save/i }))

    await waitFor(() => {
      expect(api.channels.update).toHaveBeenCalledWith(
        'reddit',
        expect.objectContaining({
          credentials: expect.objectContaining({
            subreddits: ['fern-ssg', 'staticsite'],
            user_agent: 'fern:v0.1 (by /u/fernbot)',
          }),
        })
      )
    })
  })
})
