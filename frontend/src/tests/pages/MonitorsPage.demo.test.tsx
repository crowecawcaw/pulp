import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { QueryClientProvider } from '@tanstack/react-query'
import MonitorsPage from '@/pages/MonitorsPage'
import { useWorkspaceStore } from '@/stores/workspace'
import { api } from '@/api/client'
import { ToastProvider } from '@/components/ui/toast'
import { createTestQueryClient } from '@/test/setup-utils'
import { demoMonitors, demoWorkspaces } from '@/demo/fixtures'

vi.mock('@/api/client', () => ({
  api: {
    monitors: { list: vi.fn(), create: vi.fn(), update: vi.fn(), delete: vi.fn() },
  },
  apiFetch: vi.fn(),
}))

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

// Renders the page backed by the demo monitors. The key assertion is a
// regression guard for the bug where cards showed only channel badges and not
// the monitor's keywords (terms): every term must be visible in the list.
describe('MonitorsPage (demo data)', () => {
  beforeEach(() => {
    vi.mocked(api.monitors.list).mockReset()
    vi.mocked(api.monitors.list).mockResolvedValue(demoMonitors())
    const ws = demoWorkspaces()[0]
    useWorkspaceStore.setState({ current: ws, workspaces: [ws] })
  })

  it("renders each monitor's terms, not just its channels", async () => {
    renderPage()
    await waitFor(() => {
      expect(screen.getAllByText('Nimbus').length).toBeGreaterThan(0)
    })
    // Every demo term should be on screen (mobile cards + desktop table each
    // render them, so each appears at least once).
    for (const term of ['Nimbus Labs', 'nimbusdb', 'Orrery']) {
      expect(screen.getAllByText(term).length).toBeGreaterThan(0)
    }
  })

  it('renders channel icons alongside the terms', async () => {
    renderPage()
    await waitFor(() => {
      expect(screen.getAllByText('Nimbus').length).toBeGreaterThan(0)
    })
    // Channels render as the full icon set (lit when collected, greyed when
    // not) — each icon carries its channel label as an accessible title.
    expect(screen.getAllByTitle('Reddit').length).toBeGreaterThan(0)
    expect(screen.getAllByTitle('GitHub').length).toBeGreaterThan(0)
  })
})
