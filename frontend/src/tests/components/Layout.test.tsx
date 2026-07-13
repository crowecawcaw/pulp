import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { QueryClientProvider } from '@tanstack/react-query'
import Layout from '@/components/Layout'
import { useWorkspaceStore } from '@/stores/workspace'
import { mockWorkspace } from '@/test/mocks/api'
import { api } from '@/api/client'
import { ToastProvider } from '@/components/ui/toast'
import { createTestQueryClient } from '@/test/setup-utils'

vi.mock('@/api/client', () => ({
  api: {
    workspaces: {
      list: vi.fn(),
      create: vi.fn(),
    },
  },
}))

const secondWorkspace = { ...mockWorkspace, id: 'ws-2', name: 'Second Workspace' }

const renderLayout = () =>
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <MemoryRouter>
        <ToastProvider>
          <Layout>
            <div>page content</div>
          </Layout>
        </ToastProvider>
      </MemoryRouter>
    </QueryClientProvider>,
  )

describe('Layout mobile workspace picker', () => {
  beforeEach(() => {
    useWorkspaceStore.setState({ workspaces: [mockWorkspace, secondWorkspace], current: mockWorkspace })
    vi.mocked(api.workspaces.list).mockResolvedValue([mockWorkspace, secondWorkspace])
  })

  it('opens the workspace picker directly from the top bar (no drawer/sidenav)', async () => {
    const user = userEvent.setup()
    const { container } = renderLayout()

    // No drawer/sidenav markup exists anymore
    expect(container.querySelector('.drawer')).toBeNull()

    // The mobile top-bar button shows the current workspace name
    const topbarButton = container.querySelector('.topbar__menu') as HTMLElement
    expect(topbarButton).toBeTruthy()
    expect(topbarButton).toHaveTextContent('Test Workspace')

    // Tapping it opens the workspace picker dropdown directly
    expect(container.querySelector('.ws-picker__dropdown--topbar')).toBeNull()
    await user.click(topbarButton)
    await waitFor(() => {
      expect(container.querySelector('.ws-picker__dropdown--topbar')).toBeTruthy()
    })

    // Both workspaces are listed as options inside the top-bar picker
    const topbar = container.querySelector('.topbar') as HTMLElement
    expect(within(topbar).getByRole('button', { name: 'Second Workspace' })).toBeInTheDocument()
  })

  it('switches the current workspace when an option is tapped', async () => {
    const user = userEvent.setup()
    const { container } = renderLayout()

    await user.click(container.querySelector('.topbar__menu') as HTMLElement)
    const topbar = container.querySelector('.topbar') as HTMLElement
    await user.click(await within(topbar).findByRole('button', { name: 'Second Workspace' }))

    expect(useWorkspaceStore.getState().current?.id).toBe('ws-2')
    // Dropdown closes after selection
    await waitFor(() => {
      expect(container.querySelector('.ws-picker__dropdown--topbar')).toBeNull()
    })
  })
})
