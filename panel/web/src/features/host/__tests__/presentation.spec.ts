import { describe, expect, it } from 'vitest'
import type { HostSummaryView } from '@/api/generated'
import { levelTone, memoryUsed, uptimeParts } from '../presentation'

describe('host figures', () => {
  it('read full filesystems as warnings and critical ones as negative', () => {
    expect(levelTone('critical')).toBe('negative')
    expect(levelTone('warning')).toBe('warning')
    expect(levelTone('ok')).toBe('positive')
  })

  it('give uptime in its two largest units', () => {
    expect(uptimeParts(90_000)).toEqual([
      { unit: 'days', n: 1 },
      { unit: 'hours', n: 1 },
    ])
    expect(uptimeParts(2 * 86_400)).toEqual([{ unit: 'days', n: 2 }])
    expect(uptimeParts(3_660)).toEqual([
      { unit: 'hours', n: 1 },
      { unit: 'minutes', n: 1 },
    ])
    expect(uptimeParts(59)).toEqual([{ unit: 'minutes', n: 0 }])
  })

  it('read memory in use as a share', () => {
    const host = { memory_total_bytes: 8, memory_available_bytes: 2 } as HostSummaryView
    expect(memoryUsed(host)).toBe(0.75)
    expect(memoryUsed({ ...host, memory_total_bytes: 0 })).toBe(0)
  })
})
