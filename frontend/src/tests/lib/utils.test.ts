import { describe, it, expect } from 'vitest'
import { cn } from '@/lib/utils'

describe('cn', () => {
  it('merges class names', () => {
    expect(cn('foo', 'bar')).toBe('foo bar')
  })

  it('handles conditional classes', () => {
    const condition = false
    expect(cn('base', condition && 'skipped', 'included')).toBe('base included')
  })

  it('deduplicates tailwind classes (last wins)', () => {
    const result = cn('p-2', 'p-4')
    expect(result).toBe('p-4')
  })

  it('handles undefined and null', () => {
    expect(cn('base', undefined, null)).toBe('base')
  })
})
