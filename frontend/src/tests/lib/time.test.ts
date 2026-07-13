import { describe, it, expect } from 'vitest'
import { relativeTime } from '@/lib/time'

describe('relativeTime', () => {
  const now = Math.floor(Date.now() / 1000)

  it('returns "just now" for very recent timestamps', () => {
    expect(relativeTime(now - 10)).toBe('just now')
  })

  it('returns minutes ago', () => {
    expect(relativeTime(now - 120)).toMatch(/\d+ min ago/)
  })

  it('returns hours ago', () => {
    expect(relativeTime(now - 7200)).toMatch(/\d+h ago/)
  })

  it('returns days ago', () => {
    expect(relativeTime(now - 172800)).toMatch(/\d+d ago/)
  })

  it('handles null', () => {
    expect(relativeTime(null)).toBe('unknown')
  })

  it('handles undefined', () => {
    expect(relativeTime(undefined)).toBe('unknown')
  })
})
