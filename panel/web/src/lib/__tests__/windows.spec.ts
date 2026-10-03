import { describe, expect, it } from 'vitest'
import { describeWindow, timeZones, windowForm, windowInput } from '@/lib/windows'

const SUNDAY = new Date('2026-10-04T12:00:00Z')
const name = (day: string) => day

describe('time windows', () => {
  it('writes weekly hours as RFC 5545 recurrences that already began', () => {
    expect(
      windowInput(
        { days: ['mon', 'tue'], start: '09:00', end: '18:00', timeZone: 'Europe/Berlin' },
        SUNDAY,
      ),
    ).toEqual({
      recurrence: 'DTSTART;TZID=Europe/Berlin:20260929T090000\nRRULE:FREQ=WEEKLY;BYDAY=MO,TU',
      minutes: 540,
    })
    expect(
      windowInput({ days: [], start: '22:00', end: '02:00', timeZone: 'UTC' }, SUNDAY),
    ).toEqual({ recurrence: 'DTSTART:20261003T220000Z\nRRULE:FREQ=DAILY', minutes: 240 })
  })

  it('reads its own windows back and keeps others as they are', () => {
    const office = {
      recurrence: 'DTSTART;TZID=Asia/Shanghai:20260105T090000\nRRULE:FREQ=WEEKLY;BYDAY=FR,MO',
      minutes: 540,
    }
    expect(windowForm(office)).toEqual({
      days: ['mon', 'fri'],
      start: '09:00',
      end: '18:00',
      timeZone: 'Asia/Shanghai',
    })
    expect(describeWindow(office, name)).toBe('mon fri 09:00–18:00 Asia/Shanghai')
    expect(
      describeWindow(
        { recurrence: 'DTSTART:20260101T230000Z\nRRULE:FREQ=DAILY', minutes: 120 },
        name,
      ),
    ).toBe('23:00–01:00 UTC')

    const monthly = { recurrence: 'DTSTART:20260101T000000Z\nRRULE:FREQ=MONTHLY', minutes: 60 }
    expect(windowForm(monthly).custom).toEqual(monthly)
    expect(windowInput(windowForm(monthly))).toEqual(monthly)
    expect(describeWindow(monthly, name)).toBe(
      'DTSTART:20260101T000000Z RRULE:FREQ=MONTHLY · 60 min',
    )
  })

  it('offers UTC and the IANA zones', () => {
    const zones = timeZones()
    expect(zones[0]).toBe('UTC')
    expect(zones).toContain('Europe/Berlin')
  })
})
