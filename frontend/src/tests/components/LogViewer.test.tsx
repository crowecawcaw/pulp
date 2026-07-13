import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClientProvider } from '@tanstack/react-query'
import LogViewer from '@/components/LogViewer'
import { api } from '@/api/client'
import { createTestQueryClient } from '@/test/setup-utils'

vi.mock('@/api/client', () => {
  return {
    api: {
      logs: {
        get: vi.fn(),
      },
    },
    apiFetch: vi.fn(),
  }
})

function renderViewer(ui: React.ReactElement) {
  return render(
    <QueryClientProvider client={createTestQueryClient()}>{ui}</QueryClientProvider>
  )
}

describe('LogViewer', () => {
  beforeEach(() => {
    vi.mocked(api.logs.get).mockReset()
  })
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('fetches and renders log lines for the given service', async () => {
    vi.mocked(api.logs.get).mockResolvedValue({
      service: 'reddit',
      exists: true,
      lines: ['line one for reddit', 'line two for reddit'],
    })

    renderViewer(<LogViewer service="reddit" />)

    expect(await screen.findByText('line one for reddit')).toBeInTheDocument()
    expect(screen.getByText('line two for reddit')).toBeInTheDocument()
    // Called with the service prop.
    expect(api.logs.get).toHaveBeenCalledWith('reddit', 200)
  })

  it('shows an empty-state message when there are no lines', async () => {
    vi.mocked(api.logs.get).mockResolvedValue({ service: 'github', exists: false, lines: [] })
    renderViewer(<LogViewer service="github" />)
    expect(await screen.findByText(/no log output/i)).toBeInTheDocument()
  })

  it('shows an error state when the fetch fails', async () => {
    vi.mocked(api.logs.get).mockRejectedValue(new Error('boom'))
    renderViewer(<LogViewer service="reddit" />)
    expect(await screen.findByText('boom')).toBeInTheDocument()
  })

  it('refetches when the manual refresh button is clicked', async () => {
    vi.mocked(api.logs.get).mockResolvedValue({ service: 'reddit', exists: true, lines: ['a'] })
    renderViewer(<LogViewer service="reddit" />)
    await screen.findByText('a')
    const initial = vi.mocked(api.logs.get).mock.calls.length

    await userEvent.click(screen.getByRole('button', { name: /refresh/i }))
    await waitFor(() =>
      expect(vi.mocked(api.logs.get).mock.calls.length).toBeGreaterThan(initial),
    )
  })

  it('is generic: passes a future service id (ai_filter) straight through', async () => {
    vi.mocked(api.logs.get).mockResolvedValue({ service: 'ai_filter', exists: true, lines: ['judged'] })
    renderViewer(<LogViewer service="ai_filter" />)
    await screen.findByText('judged')
    expect(api.logs.get).toHaveBeenCalledWith('ai_filter', 200)
  })
})
