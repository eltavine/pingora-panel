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
