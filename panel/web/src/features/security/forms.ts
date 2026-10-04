import type { RateLimit, RateLimitKey, SecurityPolicy, SecurityPolicyView } from '@/api/generated'
import { optionalNumber, optionalText } from '@/lib/forms'

export const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS'] as const
export const KEY_KINDS: readonly RateLimitKey['kind'][] = [
  'client_address',
  'host',
  'route',
  'header',
]
/** Rate limit periods offered by name, in seconds. */
export const PERIODS = [
  { seconds: 1, name: 'second' },
  { seconds: 60, name: 'minute' },
  { seconds: 3600, name: 'hour' },
  { seconds: 86_400, name: 'day' },
] as const

export interface RateLimitForm {
  requests: number | string
  perSeconds: number
  burst: number | string
  key: RateLimitKey['kind']
  header: string
}

export interface PolicyForm {
  id: string
  allowedCidrs: string
  deniedCidrs: string
  methods: string[]
  deniedPaths: string
  deniedUserAgents: string
  referer: boolean
  refererHosts: string
  allowEmptyReferer: boolean
  basicAuth: boolean
  usersFile: string
  realm: string
  maxHeaderSize: string
  maxBodySize: string
  bodyTimeout: number | string
  rateLimits: RateLimitForm[]
  maxConcurrent: number | string
  limited: boolean
  limitedStatus: number | string
  limitedBody: string
  limitedType: string
}

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

export function rateLimitForm(limit?: RateLimit): RateLimitForm {
  return {
    requests: limit?.requests ?? 10,
    perSeconds: limit?.per_seconds ?? 1,
    burst: limit?.burst ?? '',
    key: limit?.key.kind ?? 'client_address',
    header: limit?.key.kind === 'header' ? limit.key.name : '',
  }
}

function rateLimitKey(form: RateLimitForm): RateLimitKey {
  return form.key === 'header' ? { kind: 'header', name: form.header.trim() } : { kind: form.key }
}

export function policyForm(policy?: SecurityPolicyView): PolicyForm {
  return {
    id: policy?.id ?? '',
    allowedCidrs: (policy?.allowed_cidrs ?? []).join('\n'),
    deniedCidrs: (policy?.denied_cidrs ?? []).join('\n'),
    methods: [...(policy?.allowed_methods ?? [])],
    deniedPaths: (policy?.denied_path_prefixes ?? []).join('\n'),
    deniedUserAgents: (policy?.denied_user_agents ?? []).join('\n'),
    referer: Boolean(policy?.referer),
    refererHosts: (policy?.referer?.allowed_hosts ?? []).join('\n'),
    allowEmptyReferer: policy?.referer?.allow_empty ?? true,
    basicAuth: Boolean(policy?.basic_auth),
    usersFile: policy?.basic_auth?.users_secret_id ?? '',
    realm: policy?.basic_auth?.realm ?? 'Restricted',
    maxHeaderSize: printSize(policy?.max_header_bytes),
    maxBodySize: printSize(policy?.max_body_bytes),
    bodyTimeout: policy?.body_timeout_seconds ?? '',
    rateLimits: (policy?.rate_limits ?? []).map(rateLimitForm),
    maxConcurrent: policy?.max_concurrent_requests ?? '',
    limited: Boolean(policy?.limited_response),
    limitedStatus: policy?.limited_response?.status ?? 503,
    limitedBody: policy?.limited_response?.body ?? '',
    limitedType: policy?.limited_response?.content_type ?? '',
  }
}

export function policyBody(form: PolicyForm): SecurityPolicy {
  return {
    id: form.id.trim(),
    allowed_cidrs: words(form.allowedCidrs),
    denied_cidrs: words(form.deniedCidrs),
    allowed_methods: [...form.methods],
    denied_path_prefixes: words(form.deniedPaths),
    denied_user_agents: lines(form.deniedUserAgents),
    referer: form.referer
      ? { allowed_hosts: words(form.refererHosts), allow_empty: form.allowEmptyReferer }
      : null,
    basic_auth: form.basicAuth
      ? { realm: form.realm.trim(), users_secret_id: form.usersFile.trim() }
      : null,
    max_header_bytes: parseSize(form.maxHeaderSize),
    max_body_bytes: parseSize(form.maxBodySize),
    body_timeout_seconds: optionalNumber(form.bodyTimeout),
    rate_limits: form.rateLimits.map((limit) => ({
      key: rateLimitKey(limit),
      requests: optionalNumber(limit.requests) ?? 0,
      per_seconds: limit.perSeconds,
      burst: optionalNumber(limit.burst) ?? 0,
    })),
    max_concurrent_requests: optionalNumber(form.maxConcurrent),
    limited_response: form.limited
      ? {
          status: optionalNumber(form.limitedStatus) ?? 429,
          body: form.limitedBody,
          content_type: optionalText(form.limitedType),
        }
      : null,
  }
}

/** What a policy checks, as message keys under `security.restrictions`. */
export function restrictions(policy: SecurityPolicy): string[] {
  const checks: [boolean, string][] = [
    [(policy.allowed_cidrs?.length ?? 0) + (policy.denied_cidrs?.length ?? 0) > 0, 'networks'],
    [(policy.allowed_methods?.length ?? 0) > 0, 'methods'],
    [(policy.denied_path_prefixes?.length ?? 0) > 0, 'paths'],
    [(policy.denied_user_agents?.length ?? 0) > 0, 'userAgents'],
    [Boolean(policy.referer), 'referers'],
    [Boolean(policy.basic_auth), 'password'],
    [policy.max_header_bytes != null || policy.max_body_bytes != null, 'sizes'],
    [policy.body_timeout_seconds != null, 'bodyTimeout'],
    [(policy.rate_limits?.length ?? 0) > 0, 'rates'],
    [policy.max_concurrent_requests != null, 'concurrency'],
  ]
  return checks.filter(([present]) => present).map(([, name]) => name)
}
