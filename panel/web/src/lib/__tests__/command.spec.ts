import { describe, expect, it } from 'vitest'
import { commandHeaders, newIdempotencyKey } from '../command'

describe('commandHeaders', () => {
  it('sets an absolute RFC 3339 UTC deadline after the given instant', () => {
    const now = Date.UTC(2026, 9, 3, 4, 0, 0)
    const headers = commandHeaders('key-1', 30_000, now)

    expect(headers['x-deadline']).toBe('2026-10-03T04:00:30.000Z')
    expect(headers['Idempotency-Key']).toBe('key-1')
    // The actor is the logged-in account, never a header the console sets.
    expect(Object.keys(headers)).not.toContain('x-actor')
  })

  it('generates distinct visible-ASCII idempotency keys', () => {
    const first = newIdempotencyKey()
    const second = newIdempotencyKey()

    expect(first).not.toBe(second)
    expect(first).toMatch(/^[\x21-\x7e]{1,256}$/)
  })
})
