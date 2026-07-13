import { describe, it, expect, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import MentionCard from '@/components/MentionCard'
import { mockMention } from '@/test/mocks/api'
import { MemoryRouter } from 'react-router-dom'

const wrapper = ({ children }: { children: React.ReactNode }) => (
  <MemoryRouter>{children}</MemoryRouter>
)

describe('MentionCard', () => {
  it('renders channel name', () => {
    render(<MentionCard mention={mockMention} />, { wrapper })
    // Channel is shown as abbreviated badge (hn) with full name in title attribute
    expect(screen.getByTitle(/hackernews/i)).toBeInTheDocument()
  })

  it('renders content text', () => {
    render(<MentionCard mention={mockMention} />, { wrapper })
    expect(screen.getByText(/testbrand is a great tool/i)).toBeInTheDocument()
  })

  it('renders markdown bold instead of literal asterisks', () => {
    const m = { ...mockMention, content_text: 'I need **help** now' }
    const { container } = render(<MentionCard mention={m} />, { wrapper })
    expect(container.querySelector('strong')?.textContent).toBe('help')
    expect(container.textContent).not.toContain('**')
  })

  it('renders author name when present', () => {
    render(<MentionCard mention={mockMention} />, { wrapper })
    expect(screen.getByText(/testuser/i)).toBeInTheDocument()
  })

  it('renders external link', () => {
    render(<MentionCard mention={mockMention} />, { wrapper })
    const link = screen.getByRole('link')
    expect(link).toHaveAttribute('href', mockMention.content_url)
  })

  it('shows "Mark read" for an unread mention and fires onToggleRead', async () => {
    const onToggleRead = vi.fn()
    render(<MentionCard mention={mockMention} onToggleRead={onToggleRead} />, { wrapper })
    const btn = screen.getByRole('button', { name: /mark read/i })
    await userEvent.click(btn)
    expect(onToggleRead).toHaveBeenCalledWith(mockMention)
  })

  it('shows "Unread" affordance for a read mention', () => {
    const read = { ...mockMention, read_at: 1705002000 }
    render(<MentionCard mention={read} onToggleRead={() => {}} />, { wrapper })
    expect(screen.getByRole('button', { name: /unread/i })).toBeInTheDocument()
  })

  it('does not render a read toggle when onToggleRead is omitted', () => {
    render(<MentionCard mention={mockMention} />, { wrapper })
    expect(screen.queryByRole('button', { name: /read/i })).not.toBeInTheDocument()
  })
})
