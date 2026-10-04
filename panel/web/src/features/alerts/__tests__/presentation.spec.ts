import { describe, expect, it } from 'vitest'
import type { AlertRuleView } from '@/api/generated'
import { ruleBody, ruleForm, stateKey, toneOf } from '../presentation'
import { formatMeasure } from '@/lib/alerts'

function rule(overrides: Partial<AlertRuleView> = {}): AlertRuleView {
  return {
    id: 'shop-errors',
    spec: {
      name: 'Shop errors',
      description: '',
      measure: 'server_error_ratio',
      comparison: 'above',
      threshold: 0.05,
      pending_seconds: 300,
      site: 'shop',
      route: null,
      upstream: null,
      severity: 'critical',
      enabled: true,
      channels: ['ops'],
    },
    version: 3,
    etag: '"3"',
    created_at: '2026-10-04T10:00:00Z',
    updated_at: '2026-10-04T10:00:00Z',
    state: 'inactive',
    ...overrides,
  }
}

describe('alert measures', () => {
  it('read in the units people use', () => {
    expect(formatMeasure('server_error_ratio', 0.125, 'en')).toBe('12.5%')
    expect(formatMeasure('latency_p95', 0.25, 'en')).toBe('250 ms')
    expect(formatMeasure('latency_p95', 1.5, 'en')).toBe('1.5 s')
    expect(formatMeasure('request_rate', 30, 'en')).toBe('30 req/s')
    expect(formatMeasure('open_connections', 120.4, 'en')).toBe('120')
    expect(formatMeasure('request_rate', null, 'en')).toBe('—')
  })
})

describe('alert rules', () => {
  it('read firing as negative and disabled as neither', () => {
    expect(toneOf(rule({ state: 'firing' }))).toBe('negative')
    expect(toneOf(rule({ state: 'pending' }))).toBe('warning')
    expect(toneOf(rule())).toBe('positive')
    const disabled = rule({ state: 'firing' })
    disabled.spec.enabled = false
    expect(toneOf(disabled)).toBe('neutral')
    expect(stateKey(disabled)).toBe('alerts.states.disabled')
  })

  it('edit shares as percentages and keep them as fractions', () => {
    const form = ruleForm(rule())
    expect(form.threshold).toBe(5)
    form.threshold = '12.5'
    expect(ruleBody(form).threshold).toBe(0.125)
    form.measure = 'latency_p95'
    form.threshold = 1.5
    form.route = 'checkout'
    expect(ruleBody(form)).toMatchObject({ threshold: 1.5, site: 'shop', route: 'checkout' })
  })

  it('drop scopes the measure does not read', () => {
    const form = ruleForm(rule())
    form.measure = 'open_connections'
    form.upstream = 'app'
    expect(ruleBody(form)).toMatchObject({ site: null, route: null, upstream: null })
    form.measure = 'upstream_error_ratio'
    expect(ruleBody(form)).toMatchObject({ site: null, upstream: 'app' })
    form.name = ' '
    expect(ruleBody(form).name).toBe('shop-errors')
  })
})
