import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { QueryClientProvider } from '@tanstack/react-query'
import MonitorsPage from '@/pages/MonitorsPage'
import { useWorkspaceStore } from '@/stores/workspace'
import { mockWorkspace, mockMonitor } from '@/test/mocks/api'
import { api } from '@/api/client'
import { ToastProvider } from '@/components/ui/toast'
import { createTestQueryClient } from '@/test/setup-utils'

function renderPage() {
  return render(
    <QueryClientProvider client={createTestQueryClient()}>
      <MemoryRouter>
        <ToastProvider>
          <MonitorsPage />
        </ToastProvider>
      </MemoryRouter>
    </QueryClientProvider>
  )
}

vi.mock('@/api/client', () => {
  return {
    api: {
      monitors: {
        list: vi.fn(),
        create: vi.fn(),
        update: vi.fn(),
        delete: vi.fn(),
      },
    },
    apiFetch: vi.fn(),
  }
})

// Mock window.confirm to auto-confirm deletion
vi.stubGlobal('confirm', vi.fn().mockReturnValue(true))

describe('MonitorsPage', () => {
  beforeEach(() => {
    vi.mocked(api.monitors.list).mockReset()
    vi.mocked(api.monitors.create).mockReset()
    vi.mocked(api.monitors.update).mockReset()
    vi.mocked(api.monitors.delete).mockReset()
    vi.mocked(api.monitors.list).mockResolvedValue([mockMonitor])
    vi.mocked(api.monitors.create).mockResolvedValue(mockMonitor)
    vi.mocked(api.monitors.update).mockResolvedValue(mockMonitor)
    vi.mocked(api.monitors.delete).mockResolvedValue(undefined)
    useWorkspaceStore.setState({ current: mockWorkspace, workspaces: [mockWorkspace] })
  })

  it('shows monitors for current workspace', async () => {
    renderPage()

    // The page renders a mobile card list and a desktop table; both carry the
    // monitor's terms as chips, so expect at least one match.
    await waitFor(() => {
      expect(screen.getAllByText('testbrand').length).toBeGreaterThan(0)
    })
  })

  it('opens add monitor dialog', async () => {
    const user = userEvent.setup()
    renderPage()

    await user.click(screen.getByRole('button', { name: /add monitor/i }))
    // Dialog opens — look for the keywords field or dialog title
    await waitFor(() => {
      expect(screen.getAllByText(/add monitor/i).length).toBeGreaterThan(0)
    })
    expect(screen.getByText(/match any of these/i)).toBeInTheDocument()
  })

  it('collects multiple term chips and submits them as a terms array', async () => {
    const user = userEvent.setup()
    renderPage()

    await user.click(screen.getByRole('button', { name: /add monitor/i }))
    const input = await screen.findByLabelText(/add keyword/i)

    // Commit two chips: one via Enter, one via comma.
    await user.type(input, 'alpha{Enter}')
    await user.type(input, 'beta,')

    expect(screen.getByLabelText('remove alpha')).toBeInTheDocument()
    expect(screen.getByLabelText('remove beta')).toBeInTheDocument()

    // Remove the first chip, then save.
    await user.click(screen.getByLabelText('remove alpha'))
    expect(screen.queryByLabelText('remove alpha')).not.toBeInTheDocument()

    await user.type(input, 'gamma{Enter}')
    await user.click(screen.getByRole('button', { name: /^save$/i }))

    await waitFor(() => {
      expect(api.monitors.create).toHaveBeenCalledTimes(1)
    })
    const payload = vi.mocked(api.monitors.create).mock.calls[0][0]
    expect(payload.terms).toEqual(['beta', 'gamma'])
  })

  it('calls api.monitors.delete when delete is clicked', async () => {
    const user = userEvent.setup()
    renderPage()

    await waitFor(() => screen.getAllByText('testbrand'))

    // The delete button is the last button in the page (after Add Monitor, toggle, edit).
    const allButtons = screen.getAllByRole('button')
    const deleteBtn = allButtons[allButtons.length - 1]
    await user.click(deleteBtn)

    await waitFor(() => {
      expect(api.monitors.delete).toHaveBeenCalledWith('mon-1')
    })
  })
})
