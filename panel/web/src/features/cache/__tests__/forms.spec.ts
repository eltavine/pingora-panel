import { describe, expect, it } from 'vitest'
import type { CachePolicyView } from '@/api/generated'
import { newCondition } from '@/lib/conditions'
import {
  cachePolicyBody,
  cachePolicyForm,
  lifetime,
  lookups,
  parseDuration,
  policyProblems,
  printDuration,
  statusTimes,
  storeBytes,
} from '../forms'

const policy: CachePolicyView = {
  id: 'pages',
  ttl_seconds: 600,
  status_ttls: { '404': 60, '410': 60, '500': 0 },
  key: '$scheme$host$request_uri$cookie_lang',
  vary_headers: ['accept-language'],
  honor_origin: false,
  bypass: [{ kind: 'cookie', name: 'session', test: { op: 'present' } }],
  stale_while_revalidate_seconds: 30,
  stale_if_error_seconds: 300,
  max_object_bytes: 16 * 1024 ** 2,
  status_header: false,
  used_by: [],
  etag: '"c1"',
}

describe('cache policy forms', () => {
  it('read and print durations as the configuration language writes them', () => {
    expect(parseDuration('1h30m')).toBe(5_400)
    expect(parseDuration('90')).toBe(90)
    expect(parseDuration('')).toBeNull()
    expect(parseDuration('10 minutes')).toBeNaN()
    expect(printDuration(5_400)).toBe('90m')
    expect(printDuration(86_400)).toBe('1d')
    expect(printDuration(0)).toBe('0')
    expect(printDuration(undefined)).toBe('')
  })

  it('round-trip a policy, grouping statuses that share a lifetime', () => {
    const form = cachePolicyForm(policy)
    expect(form.ttl).toBe('10m')
    expect(form.statusTtls).toEqual([
      { statuses: '404 410', ttl: '1m' },
      { statuses: '500', ttl: '0' },
    ])
    expect(form.staleIfError).toBe('5m')
    expect(form.maxObjectSize).toBe('16m')
    expect(form.bypass[0]?.name).toBe('session')
    const { used_by: _used, etag: _etag, ...stored } = policy
    expect(cachePolicyBody(form)).toEqual({ ...stored, enabled: true })
  })

  it('start new policies with what the origin says and leave empty fields out', () => {
    const form = cachePolicyForm()
    expect(form.honorOrigin && form.statusHeader && form.enabled).toBe(true)
    form.id = 'assets'
    form.vary = 'Accept-Encoding'
    expect(cachePolicyBody(form)).toMatchObject({
      id: 'assets',
      ttl_seconds: 0,
      status_ttls: {},
      key: null,
      vary_headers: ['accept-encoding'],
      max_object_bytes: null,
    })
    expect(policyProblems(form)).toEqual([])
  })

  it('tell what keeps a policy from being saved', () => {
    const form = cachePolicyForm()
    form.ttl = 'soon'
    form.statusTtls.push({ statuses: '4040', ttl: '1m' })
    form.vary = 'X/Tenant'
    form.bypass.push(newCondition('cookie'))
    form.staleIfError = 'later'
    form.maxObjectSize = '128m'
    expect(policyProblems(form)).toEqual([
      'ttl',
      'statuses',
      'vary',
      'bypass',
      'stale',
      'objectSize',
    ])
  })

  it('size the store between 1m and 64g', () => {
    expect(storeBytes('')).toBeNull()
    expect(storeBytes('512m')).toBe(512 * 1024 ** 2)
    expect(storeBytes('512k')).toBeNaN()
    expect(storeBytes('65g')).toBeNaN()
  })

  it('summarize policies and lookups', () => {
    expect(lifetime(policy)).toBe('10m')
    expect(statusTimes(policy)).toBe('404=1m 410=1m 500=0')
    expect(
      lookups({
        site_id: 'shop',
        hits: 6,
        stale: 1,
        updating: 0,
        revalidated: 1,
        misses: 2,
        expired: 0,
        uncacheable: 0,
        bypasses: 5,
        hit_ratio: 0.8,
      }),
    ).toEqual({ served: 8, total: 15 })
  })
})
