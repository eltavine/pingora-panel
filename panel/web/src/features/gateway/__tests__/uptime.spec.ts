import { describe, expect, it } from 'vitest'
import { formatUptime } from '../uptime'

describe('formatUptime', () => {
  it('shows the two largest units', () => {
    expect(formatUptime(0)).toBe('0s')
    expect(formatUptime(42)).toBe('42s')
    expect(formatUptime(312)).toBe('5m 12s')
    expect(formatUptime(3 * 86_400 + 4 * 3_600 + 59)).toBe('3d 4h')
  })

  it('drops a zero second unit', () => {
    expect(formatUptime(7_200)).toBe('2h')
    expect(formatUptime(86_400 + 30)).toBe('1d')
  })
})
