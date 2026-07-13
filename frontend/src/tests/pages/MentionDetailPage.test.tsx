import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, Routes, Route } from 'react-router-dom'
import { QueryClientProvider } from '@tanstack/react-query'
import MentionDetailPage from '@/pages/MentionDetailPage'
import { api } from '@/api/client'
import { ToastProvider } from '@/components/ui/toast'
import { createTestQueryClient } from '@/test/setup-utils'
import { mockMention } from '@/test/mocks/api'

vi.mock('@/api/client', () => ({
  api: {
    mentions: { get: vi.fn(), setRead: vi.fn() },
  },
  apiFetch: vi.fn(),
}))

function renderAt(id: string) {
  return render(
    <QueryClientProvider client={createTestQueryClient()}>
      <MemoryRouter initialEntries={[`/mentions/${id}`]}>
        <ToastProvider>
          <Routes>
            <Route path="/mentions/:id" element={<MentionDetailPage />} />
          </Routes>
        </ToastProvider>
      </MemoryRouter>
    </QueryClientProvider>,
  )
}

describe('MentionDetailPage', () => {
  beforeEach(() => {
    vi.mocked(api.mentions.get).mockReset()
    vi.mocked(api.mentions.setRead).mockReset()
  })

  it('fetches the mention by the route id and shows its content + a link to the original', async () => {
    vi.mocked(api.mentions.get).mockResolvedValue(mockMention)
    renderAt(mockMention.id)

    expect(await screen.findByText(/testbrand is a great tool/i)).toBeInTheDocument()
    expect(api.mentions.get).toHaveBeenCalledWith(mockMention.id)
    const original = screen.getByRole('link', { name: /open original/i })
    expect(original).toHaveAttribute('href', mockMention.content_url)
  })

  it('shows a not-found message when the mention does not exist', async () => {
    vi.mocked(api.mentions.get).mockRejectedValue(new Error('mention not found'))
    renderAt('missing')

    await waitFor(() => {
      expect(screen.getByText(/mention not found/i)).toBeInTheDocument()
    })
  })
})
