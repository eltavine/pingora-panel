import { describe, expect, it } from 'vitest'
import type { AuditEvent } from '@/api/generated'
import { messages } from '@/i18n/messages'
import { KNOWN_TYPES, isKnownType, summaryOf, toneOf, typeKey } from '../presentation'

function event(event_type: string, data: Record<string, unknown>): AuditEvent {
  return {
    sequence: 1,
    event_id: 'e',
    source: '/pingora-panel/config-service',
    event_type,
    event_version: 1,
    subject: 'configuration/draft',
    actor_type: 'user',
    actor_id: 'ops',
    correlation_id: 'req-1',
    causation_id: 'req-1',
    data,
    hash: 'a'.repeat(64),
    previous_hash: '',
  }
}

const t = (key: string, values?: Record<string, unknown>) =>
  `${key}${values ? JSON.stringify(values) : ''}`

describe('audit presentation', () => {
  it('names every known type in both languages', () => {
    const missing = Object.values(messages).flatMap((locale) => {
      const types = locale.audit.types as Record<string, string>
      return KNOWN_TYPES.filter((type) => !types[typeKey(type).split('.').pop()!])
    })
    expect(missing).toEqual([])
    expect(isKnownType('config.draft.changed')).toBe(true)
    expect(isKnownType('other.event')).toBe(false)
  })

  it('marks refusals and failures as negative', () => {
    expect(toneOf('config.change.refused')).toBe('negative')
    expect(toneOf('config.apply.failed')).toBe('negative')
    expect(toneOf('config.apply.rejected')).toBe('negative')
    expect(toneOf('config.draft.applied')).toBe('positive')
  })

  it('summarizes what each event did', () => {
    expect(
      summaryOf(
        event('config.draft.changed', {
          operation: 'sites.create',
          resource: 'sites',
          version: 4,
        }),
        t,
      ),
    ).toBe('sites.create sites → v4')
    expect(
      summaryOf(event('config.draft.applied', { version: 4, revision: 9, note: 'launch' }), t),
    ).toBe('v4 → #9 · launch')
    expect(
      summaryOf(event('config.apply.rejected', { version: 4, valid: false, codes: ['A', 'B'] }), t),
    ).toBe('v4 · A, B')
    expect(
      summaryOf(
        event('gateway.operation.refused', { operation: 'reloaded', code: 'UNAVAILABLE' }),
        t,
      ),
    ).toBe('reloaded · UNAVAILABLE')
    expect(summaryOf(event('gateway.reloaded', { generation: 3, workers: 2 }), t)).toBe(
      'audit.summary.generation{"generation":"3","workers":"2"}',
    )
    expect(summaryOf(event('other.event', { a: 1 }), t)).toBe('{"a":1}')
  })
})
