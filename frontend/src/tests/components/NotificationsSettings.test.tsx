import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { QueryClientProvider } from '@tanstack/react-query'
import { NotificationsSettings } from '@/components/NotificationsSettings'
import { useWorkspaceStore } from '@/stores/workspace'
import { mockWorkspace } from '@/test/mocks/api'
import { api } from '@/api/client'
import { ToastProvider } from '@/components/ui/toast'
import { createTestQueryClient } from '@/test/setup-utils'
import * as push from '@/lib/push'
import type { Notification } from '@/api/types'

vi.mock('@/api/client', () => ({
  api: {
    notifications: {
      list: vi.fn(),
      create: vi.fn(),
      delete: vi.fn(),
      test: vi.fn(),
    },
  },
  apiFetch: vi.fn(),
}))

// Push helpers touch navigator.serviceWorker / Notification which jsdom lacks,
// so mock the module. Default: web push supported, not the current device.
vi.mock('@/lib/push', () => ({
  pushSupported: vi.fn(() => true),
  isStandalone: vi.fn(() => true),
  isIos: vi.fn(() => false),
  currentPushEndpoint: vi.fn(async () => null),
  deviceLabel: vi.fn(() => 'Mac — Chrome'),
  enablePush: vi.fn(async () => ({
    endpoint: 'https://push.example.com/new-device',
    p256dh: 'p',
    auth: 'a',
  })),
}))

const webhookNotif: Notification = {
  id: 'notif-1',
  workspace_id: 'ws-1',
  kind: 'webhook',
  config: { url: 'https://hooks.example.com/services/T/B/x' },
  label: 'Team chat',
  created_at: 1705000000,
}

function renderPage() {
  return render(
    <QueryClientProvider client={createTestQueryClient()}>
      <MemoryRouter>
        <ToastProvider>
          <NotificationsSettings />
        </ToastProvider>
      </MemoryRouter>
    </QueryClientProvider>,
  )
}

describe('NotificationsSettings', () => {
  beforeEach(() => {
    vi.mocked(api.notifications.list).mockReset()
    vi.mocked(api.notifications.create).mockReset()
    vi.mocked(api.notifications.delete).mockReset()
    vi.mocked(api.notifications.test).mockReset()
    vi.mocked(api.notifications.list).mockResolvedValue([webhookNotif])
    vi.mocked(api.notifications.create).mockResolvedValue({ ...webhookNotif, id: 'notif-2' })
    vi.mocked(api.notifications.delete).mockResolvedValue(undefined)
    vi.mocked(api.notifications.test).mockResolvedValue({ delivered: 1 })
    vi.mocked(push.currentPushEndpoint).mockResolvedValue(null)
    useWorkspaceStore.setState({ current: mockWorkspace, workspaces: [mockWorkspace] })
  })

  it("renders the workspace's notifications", async () => {
    renderPage()
    await waitFor(() => {
      expect(screen.getByText('Team chat')).toBeInTheDocument()
    })
    expect(screen.getByText('(webhook)')).toBeInTheDocument()
  })

  it('enable-on-this-device subscribes and creates a webpush notification', async () => {
    const user = userEvent.setup()
    renderPage()

    const enableBtn = await screen.findByRole('button', { name: /enable on this device/i })
    await user.click(enableBtn)

    await waitFor(() => {
      expect(push.enablePush).toHaveBeenCalled()
      expect(api.notifications.create).toHaveBeenCalledWith({
        workspace_id: 'ws-1',
        kind: 'webpush',
        config: { endpoint: 'https://push.example.com/new-device', p256dh: 'p', auth: 'a' },
        label: 'Mac — Chrome',
      })
    })
  })

  it('adds a webhook notification', async () => {
    const user = userEvent.setup()
    renderPage()

    await user.click(await screen.findByRole('button', { name: /add webhook/i }))
    const input = await screen.findByPlaceholderText(/example\.com\/hook/i)
    await user.type(input, 'https://my.hook/here')
    await user.click(screen.getByRole('button', { name: /^add$/i }))

    await waitFor(() => {
      expect(api.notifications.create).toHaveBeenCalledWith({
        workspace_id: 'ws-1',
        kind: 'webhook',
        config: { url: 'https://my.hook/here' },
      })
    })
  })

  it('send test calls the test endpoint', async () => {
    const user = userEvent.setup()
    renderPage()

    const testBtn = await screen.findByRole('button', { name: /send test/i })
    await user.click(testBtn)

    await waitFor(() => {
      expect(api.notifications.test).toHaveBeenCalledWith('ws-1')
    })
  })

  it('remove calls delete', async () => {
    const user = userEvent.setup()
    renderPage()

    await waitFor(() => screen.getByText('Team chat'))
    const removeBtn = screen.getByRole('button', { name: /remove this notification/i })
    await user.click(removeBtn)

    await waitFor(() => {
      expect(api.notifications.delete).toHaveBeenCalledWith('notif-1')
    })
  })
})
