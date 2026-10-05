import { describe, expect, it } from 'vitest'
import type { SecurityPolicyView } from '@/api/generated'
import { parseSize, printSize } from '@/lib/forms'
import { policyBody, policyForm, rateLimitForm, restrictions } from '../forms'

const policy: SecurityPolicyView = {
  id: 'office',
  allowed_cidrs: ['10.0.0.0/8', '2001:db8::/32'],
  denied_cidrs: ['10.9.0.0/16'],
  allowed_methods: ['GET', 'POST'],
  denied_path_prefixes: ['/.git'],
  denied_user_agents: ['^curl/', 'bad bot'],
  referer: { allowed_hosts: ['*.example.com'], allow_empty: false },
  basic_auth: { realm: 'Staff', users_secret_id: 'staff.htpasswd' },
  max_header_bytes: 16_384,
  max_body_bytes: 10_485_760,
  body_timeout_seconds: 30,
  rate_limits: [
    { key: { kind: 'client_address' }, requests: 10, per_seconds: 1, burst: 20 },
    { key: { kind: 'header', name: 'X-Api-Key' }, requests: 5, per_seconds: 10, burst: 0 },
  ],
  max_concurrent_requests: 8,
  limited_response: { status: 503, body: 'Slow down', content_type: 'text/plain' },
  used_by: ['0d7c'],
  etag: '"p1"',
}

describe('security policy forms', () => {
  it('round-trips every setting', () => {
    const { used_by: _usedBy, etag: _etag, ...stored } = policy
    expect(policyBody(policyForm(policy))).toEqual(stored)
  })

  it('leaves unset limits and switched-off sections out', () => {
    const form = policyForm()
    form.id = ' open '
    form.referer = false
    form.basicAuth = false
    form.rateLimits.push(rateLimitForm())
    expect(policyBody(form)).toEqual({
      id: 'open',
      allowed_cidrs: [],
      denied_cidrs: [],
      allowed_methods: [],
      denied_path_prefixes: [],
      denied_user_agents: [],
      referer: null,
      basic_auth: null,
      max_header_bytes: null,
      max_body_bytes: null,
      body_timeout_seconds: null,
      rate_limits: [{ key: { kind: 'client_address' }, requests: 10, per_seconds: 1, burst: 0 }],
      max_concurrent_requests: null,
      limited_response: null,
    })
  })

  it('reads sizes as the configuration language writes them', () => {
    expect(parseSize('16k')).toBe(16_384)
    expect(parseSize(' 10M ')).toBe(10_485_760)
    expect(parseSize('512')).toBe(512)
    expect(parseSize('')).toBeNull()
    for (const invalid of ['0', '1t', 'k', '-1', '1.5m']) {
      expect(parseSize(invalid)).toBeNaN()
    }
    expect(printSize(16_384)).toBe('16k')
    expect(printSize(1_000)).toBe('1000')
    expect(printSize(null)).toBe('')
  })

  it('names what a policy restricts', () => {
    expect(restrictions(policy)).toEqual([
      'networks',
      'methods',
      'paths',
      'userAgents',
      'referers',
      'password',
      'sizes',
      'bodyTimeout',
      'rates',
      'concurrency',
    ])
    expect(restrictions({ id: 'empty' })).toEqual([])
  })
})
