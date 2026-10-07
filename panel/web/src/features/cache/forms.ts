import type { CachePolicy, CachePolicyView, SiteCacheStatsResponse } from '@/api/generated'
import { conditionForm, conditionProblem, toCondition, type ConditionForm } from '@/lib/conditions'
import { optionalText, parseSize, printSize, words } from '@/lib/forms'

/** What tells stored responses apart unless a policy says. */
export const DEFAULT_KEY = '$scheme$host$request_uri'
/** What the store keeps unless set. */
export const DEFAULT_STORE_BYTES = 256 * 1024 ** 2
export const LEAST_STORE_BYTES = 1024 ** 2
export const MOST_STORE_BYTES = 64 * 1024 ** 3
export const MOST_OBJECT_BYTES = 64 * 1024 ** 2

/** RFC 9110 §5.6.2 token characters, which field names are made of. */
const TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/

const DURATION_UNITS = [
  ['d', 86_400],
  ['h', 3_600],
  ['m', 60],
  ['s', 1],
] as const

/** Seconds from `30s`, `10m`, `1h30m`, `2d` or a bare number; `null` when empty, `NaN` when invalid. */
export function parseDuration(value: string): number | null {
  const text = value.trim().toLowerCase()
  if (text === '') {
    return null
  }
  if (/^\d+$/.test(text)) {
    return Number(text)
  }
  if (!/^(\d+d)?(\d+h)?(\d+m)?(\d+s)?$/.test(text)) {
    return Number.NaN
  }
  let total = 0
  for (const [, amount, unit] of text.matchAll(/(\d+)([dhms])/g)) {
    total += Number(amount) * DURATION_UNITS.find(([name]) => name === unit)![1]
  }
  return Number.isSafeInteger(total) ? total : Number.NaN
}

/** The largest unit that states `seconds` exactly; `0` as it is. */
export function printDuration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) {
    return ''
  }
  if (seconds === 0) {
    return '0'
  }
  const [unit, factor] = DURATION_UNITS.find(([, factor]) => seconds % factor === 0)!
  return `${seconds / factor}${unit}`
}

/** Statuses sharing one lifetime, such as `404 410` for `1m`. */
export interface StatusTtlForm {
  statuses: string
  ttl: string
}

export interface CachePolicyForm {
  id: string
  enabled: boolean
  ttl: string
  statusTtls: StatusTtlForm[]
  key: string
  vary: string
  honorOrigin: boolean
  bypass: ConditionForm[]
  staleWhileRevalidate: string
  staleIfError: string
  maxObjectSize: string
  statusHeader: boolean
}

export function cachePolicyForm(policy?: CachePolicyView): CachePolicyForm {
  const grouped = new Map<number, string[]>()
  for (const [status, seconds] of Object.entries(policy?.status_ttls ?? {})) {
    grouped.set(seconds, [...(grouped.get(seconds) ?? []), status])
  }
  return {
    id: policy?.id ?? '',
    enabled: policy?.enabled ?? true,
    ttl: printDuration(policy?.ttl_seconds || null),
    statusTtls: [...grouped].map(([seconds, statuses]) => ({
      statuses: statuses.join(' '),
      ttl: printDuration(seconds),
    })),
    key: policy?.key ?? '',
    vary: (policy?.vary_headers ?? []).join(' '),
    honorOrigin: policy?.honor_origin ?? true,
    bypass: (policy?.bypass ?? []).map(conditionForm),
    staleWhileRevalidate: printDuration(policy?.stale_while_revalidate_seconds || null),
    staleIfError: printDuration(policy?.stale_if_error_seconds || null),
    maxObjectSize: printSize(policy?.max_object_bytes),
    statusHeader: policy?.status_header ?? true,
  }
}

export function cachePolicyBody(form: CachePolicyForm): CachePolicy {
  const statusTtls: Record<string, number> = {}
  for (const row of form.statusTtls) {
    const seconds = parseDuration(row.ttl)
    if (seconds === null || Number.isNaN(seconds)) {
      continue
    }
    for (const status of words(row.statuses)) {
      statusTtls[status] = seconds
    }
  }
  return {
    id: form.id.trim(),
    enabled: form.enabled,
    ttl_seconds: parseDuration(form.ttl) ?? 0,
    status_ttls: statusTtls,
    key: optionalText(form.key),
    vary_headers: words(form.vary).map((name) => name.toLowerCase()),
    honor_origin: form.honorOrigin,
    bypass: form.bypass.map(toCondition),
    stale_while_revalidate_seconds: parseDuration(form.staleWhileRevalidate) ?? 0,
    stale_if_error_seconds: parseDuration(form.staleIfError) ?? 0,
    max_object_bytes: parseSize(form.maxObjectSize),
    status_header: form.statusHeader,
  }
}

const invalid = (value: string) => Number.isNaN(parseDuration(value))

function statusesInvalid(row: StatusTtlForm): boolean {
  const statuses = words(row.statuses)
  return (
    statuses.length === 0 ||
    statuses.some((status) => !/^[1-5]\d\d$/.test(status)) ||
    parseDuration(row.ttl) === null ||
    invalid(row.ttl)
  )
}

/** What keeps the form from being saved, as message keys under `cache.problems`. */
export function policyProblems(form: CachePolicyForm): string[] {
  const size = parseSize(form.maxObjectSize)
  const checks: [boolean, string][] = [
    [invalid(form.ttl), 'ttl'],
    [form.statusTtls.some(statusesInvalid), 'statuses'],
    [words(form.vary).some((name) => !TOKEN.test(name)), 'vary'],
    [form.bypass.some((condition) => conditionProblem(condition)), 'bypass'],
    [invalid(form.staleWhileRevalidate) || invalid(form.staleIfError), 'stale'],
    [size !== null && (Number.isNaN(size) || size > MOST_OBJECT_BYTES), 'objectSize'],
  ]
  return checks.filter(([found]) => found).map(([, key]) => key)
}

/** A store size as written; `null` for the default, `NaN` when out of bounds. */
export function storeBytes(value: string): number | null {
  const bytes = parseSize(value)
  if (bytes === null || Number.isNaN(bytes)) {
    return bytes
  }
  return bytes >= LEAST_STORE_BYTES && bytes <= MOST_STORE_BYTES ? bytes : Number.NaN
}

/** Lookups a site's responses came from the cache in, of all it made. */
export function lookups(site: SiteCacheStatsResponse): { served: number; total: number } {
  const served = site.hits + site.stale + site.updating + site.revalidated
  return { served, total: served + site.misses + site.expired + site.uncacheable + site.bypasses }
}

/** What a policy keeps responses for, in a few words for the list. */
export function lifetime(policy: CachePolicy): string {
  return printDuration(policy.ttl_seconds || null)
}

/** The statuses a policy times on its own, such as `404=1m 500=0`. */
export function statusTimes(policy: CachePolicy): string {
  return Object.entries(policy.status_ttls ?? {})
    .map(([status, seconds]) => `${status}=${printDuration(seconds)}`)
    .join(' ')
}
