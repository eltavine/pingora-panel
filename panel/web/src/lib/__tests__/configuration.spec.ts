import { describe, expect, it } from 'vitest'
import { changeHeaders, plainHeaders } from '../configuration'

describe('change headers', () => {
  it('sends If-Match only for conditional changes', () => {
    expect(changeHeaders('"v1"')['If-Match']).toBe('"v1"')
    expect(plainHeaders()).not.toHaveProperty('If-Match')
  })

  it('uses a fresh idempotency key per change', () => {
    expect(plainHeaders()['Idempotency-Key']).not.toBe(plainHeaders()['Idempotency-Key'])
  })
})
