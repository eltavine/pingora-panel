import type { RateLimit, RateLimitKey, SecurityPolicy, SecurityPolicyView } from '@/api/generated'
import { lines, optionalNumber, optionalText, parseSize, printSize, words } from '@/lib/forms'

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
