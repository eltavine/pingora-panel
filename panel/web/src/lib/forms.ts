import type { RealIpHeader } from '@/api/generated'

/** Empty text is absent, not an empty value. */
export function optionalText(value: string): string | null {
  const trimmed = value.trim()
  return trimmed === '' ? null : trimmed
}

/** Empty or non-numeric input is absent. */
export function optionalNumber(value: number | string | null | undefined): number | null {
  if (value === null || value === undefined || value === '') {
    return null
  }
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : null
}

export const RESOURCE_ID = /^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/

export const REAL_IP_HEADERS: readonly RealIpHeader[] = [
  'x-forwarded-for',
  'x-real-ip',
  'forwarded',
]

const SIZE_UNITS = [
  ['g', 1024 ** 3],
  ['m', 1024 ** 2],
  ['k', 1024],
] as const

/** Bytes from `16k`, `10m`, `1g` or a bare number; `null` when empty, `NaN` when invalid. */
export function parseSize(value: string): number | null {
  const text = value.trim().toLowerCase()
  if (text === '') {
    return null
  }
  const match = /^(\d+)([kmg]?)$/.exec(text)
  if (!match) {
    return Number.NaN
  }
  const factor = SIZE_UNITS.find(([unit]) => unit === match[2])?.[1] ?? 1
  const bytes = Number(match[1]) * factor
  return bytes > 0 && Number.isSafeInteger(bytes) ? bytes : Number.NaN
}

export function printSize(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined) {
    return ''
  }
  const unit = SIZE_UNITS.find(([, factor]) => bytes % factor === 0)
  return unit ? `${bytes / unit[1]}${unit[0]}` : String(bytes)
}

/** One entry per line or separated by spaces and commas. */
export function words(value: string): string[] {
  return value
    .split(/[\s,]+/)
    .map((item) => item.trim())
    .filter((item) => item.length > 0)
}

/** One entry per line, for values that may hold spaces such as patterns. */
export function lines(value: string): string[] {
  return value
    .split('\n')
    .map((item) => item.trim())
    .filter((item) => item.length > 0)
}
