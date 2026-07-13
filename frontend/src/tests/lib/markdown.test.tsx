import { describe, it, expect } from 'vitest'
import { render } from '@testing-library/react'
import { renderMarkdown } from '@/lib/markdown'

function html(text: string): string {
  const { container } = render(<div>{renderMarkdown(text)}</div>)
  return container.innerHTML
}

describe('renderMarkdown', () => {
  it('renders bold with **', () => {
    expect(html('a **bold** b')).toBe('<div>a <strong>bold</strong> b</div>')
  })

  it('renders bold with __', () => {
    expect(html('__bold__')).toBe('<div><strong>bold</strong></div>')
  })

  it('renders italic with *', () => {
    expect(html('an *em* word')).toBe('<div>an <em>em</em> word</div>')
  })

  it('does not treat single underscores as italics (usernames/urls)', () => {
    expect(html('user_name_here')).toBe('<div>user_name_here</div>')
  })

  it('renders inline code', () => {
    const out = html('run `npm test`')
    expect(out).toContain('<code')
    expect(out).toContain('npm test')
  })

  it('renders links with safe rel/target', () => {
    const out = html('see [docs](https://example.com/x)')
    expect(out).toContain('href="https://example.com/x"')
    expect(out).toContain('target="_blank"')
    expect(out).toContain('rel="noopener noreferrer"')
    expect(out).toContain('>docs</a>')
  })

  it('leaves plain text untouched', () => {
    expect(html('just text, no markup')).toBe('<div>just text, no markup</div>')
  })

  it('handles bold spanning a whole post', () => {
    expect(html('**Comet AI is a browser. I need help.**')).toBe(
      '<div><strong>Comet AI is a browser. I need help.</strong></div>',
    )
  })
})
