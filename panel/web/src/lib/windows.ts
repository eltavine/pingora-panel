import type { Window } from '@/api/generated'

/**
 * Time windows as forms edit them: days of the week and hours in a time
 * zone, kept by the API as RFC 5545 recurrences. Windows written another
 * way, through the API or the CLI, stay as they are.
 */

export const DAYS = ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'] as const
export type DayName = (typeof DAYS)[number]

const CODES: Record<DayName, string> = {
  mon: 'MO',
  tue: 'TU',
  wed: 'WE',
  thu: 'TH',
  fri: 'FR',
  sat: 'SA',
  sun: 'SU',
}
const DAY_MINUTES = 24 * 60

export interface WindowForm {
  days: DayName[]
  start: string
  end: string
  timeZone: string
  /** A window written another way, kept unchanged. */
  custom?: Window
}

/** The browser's time zone. */
export function localTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
}

/** The IANA time zones the browser knows, UTC first. */
export function timeZones(): string[] {
  return ['UTC', ...Intl.supportedValuesOf('timeZone').filter((zone) => zone !== 'UTC')]
}

export function blankWindow(): WindowForm {
  return { days: [], start: '09:00', end: '18:00', timeZone: localTimeZone() }
}

function clock(minutes: number): string {
  const time = ((minutes % DAY_MINUTES) + DAY_MINUTES) % DAY_MINUTES
  return `${String(Math.floor(time / 60)).padStart(2, '0')}:${String(time % 60).padStart(2, '0')}`
}

function minutesOf(time: string): number {
  const [hours = 0, minutes = 0] = time.split(':').map(Number)
  return hours * 60 + minutes
}

/** The zone, start and days of a recurrence of weekly hours. */
function weekly(recurrence: string): { zone: string; start: number; days: DayName[] } | null {
  const [first = '', rule = '', ...rest] = recurrence.split('\n')
  const zoned = /^DTSTART;TZID=([^:]+):\d{8}T(\d{2})(\d{2})\d{2}$/.exec(first)
  const utc = /^DTSTART:\d{8}T(\d{2})(\d{2})\d{2}Z$/.exec(first)
  const [zone, hours, minutes] = zoned
    ? [zoned[1], zoned[2], zoned[3]]
    : utc
      ? ['UTC', utc[1], utc[2]]
      : []
  const byDay = /^RRULE:FREQ=WEEKLY;BYDAY=([A-Z,]+)$/.exec(rule)?.[1]?.split(',')
  if (!zone || rest.length || (rule !== 'RRULE:FREQ=DAILY' && !byDay)) {
    return null
  }
  const days = DAYS.filter((day) => byDay?.includes(CODES[day]))
  if (byDay && days.length !== byDay.length) {
    return null
  }
  return { zone, start: Number(hours) * 60 + Number(minutes), days }
}

/** The form of `window`, custom when it is not weekly hours. */
export function windowForm(window: Window): WindowForm {
  const parsed = weekly(window.recurrence)
  if (!parsed) {
    return { ...blankWindow(), custom: window }
  }
  return {
    days: parsed.days,
    start: clock(parsed.start),
    end: clock(parsed.start + window.minutes),
    timeZone: parsed.zone,
  }
}

/**
 * The window a form describes. Its first period starts on the latest
 * matching day at least a day before `today`, so it has begun in every time
 * zone; an end at or before the start is on the next day.
 */
export function windowInput(form: WindowForm, today = new Date()): Window {
  if (form.custom) {
    return form.custom
  }
  const start = minutesOf(form.start)
  const end = minutesOf(form.end)
  const days = DAYS.filter((day) => form.days.includes(day))
  const date = new Date(
    Date.UTC(today.getUTCFullYear(), today.getUTCMonth(), today.getUTCDate() - 1),
  )
  while (days.length && !days.includes(DAYS[(date.getUTCDay() + 6) % 7]!)) {
    date.setUTCDate(date.getUTCDate() - 1)
  }
  const stamp = `${date.toISOString().slice(0, 10).replaceAll('-', '')}T${clock(start).replace(':', '')}00`
  const dtstart =
    form.timeZone === 'UTC' ? `DTSTART:${stamp}Z` : `DTSTART;TZID=${form.timeZone}:${stamp}`
  const rule = days.length
    ? `FREQ=WEEKLY;BYDAY=${days.map((day) => CODES[day]).join(',')}`
    : 'FREQ=DAILY'
  return {
    recurrence: `${dtstart}\nRRULE:${rule}`,
    minutes: end > start ? end - start : end + DAY_MINUTES - start,
  }
}

/** A window as people read it. */
export function describeWindow(window: Window, dayName: (day: DayName) => string): string {
  const form = windowForm(window)
  if (form.custom) {
    return `${window.recurrence.replace('\n', ' ')} · ${window.minutes} min`
  }
  const days = form.days.map(dayName).join(' ')
  return `${days ? `${days} ` : ''}${form.start}–${form.end} ${form.timeZone}`
}
