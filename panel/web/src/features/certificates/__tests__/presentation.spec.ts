import { describe, expect, it } from 'vitest'
import { CERTIFICATE_ID, daysLeft, fingerprint, parseNames, suggestedId } from '../presentation'

describe('certificate presentation', () => {
  it('accepts the identifiers the inventory accepts', () => {
    const valid = ['example.com', 'a', 'intranet-wildcard', '0.example']
    const invalid = ['', 'Example.com', 'a..b', '.a', 'a.', '-a', 'a-', 'a_b', 'x'.repeat(65)]
    expect(valid.filter((id) => !CERTIFICATE_ID.test(id))).toEqual([])
    expect(invalid.filter((id) => CERTIFICATE_ID.test(id))).toEqual([])
  })

  it('shows fingerprints as browsers do', () => {
    expect(fingerprint('0a1bff')).toBe('0A:1B:FF')
    expect(fingerprint('')).toBe('')
  })

  it('counts whole days left', () => {
    const now = Date.parse('2026-10-03T00:00:00Z')
    expect(daysLeft('2026-10-13T12:00:00Z', now)).toBe(10)
    expect(daysLeft('2026-10-02T12:00:00Z', now)).toBe(-1)
  })

  it('reads names and suggests an identifier', () => {
    const names = parseNames('*.Example.com\nexample.com, 10.0.0.1  example.com')
    expect(names).toEqual(['*.Example.com', 'example.com', '10.0.0.1'])
    expect(suggestedId(names)).toBe('example.com')
    expect(suggestedId(['10.0.0.1'])).toBe('10.0.0.1')
    expect(suggestedId(['bad_name.example'])).toBe('')
  })
})
