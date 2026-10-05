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
    expect(toneOf('identity.access.denied')).toBe('negative')
    expect(toneOf('identity.login.succeeded')).toBe('positive')
  })

  it('summarizes logins, account changes and refused access', () => {
    expect(
      summaryOf(
        event('identity.login.failed', {
          attempt: { username: 'root', client_address: '192.0.2.7' },
          reason: 'wrong_password',
        }),
        t,
      ),
    ).toBe('root · 192.0.2.7 · wrong_password')
    expect(
      summaryOf(
        event('identity.account.updated', { disabled: true, roles: { names: ['viewer'] } }),
        t,
      ),
    ).toBe('audit.summary.disabled · viewer')
    expect(
      summaryOf(
        event('identity.access.denied', {
          method: 'GET',
          route: '/api/v1/accounts',
          reason: 'permission',
          permission: 'identity.read',
        }),
        t,
      ),
    ).toBe('GET /api/v1/accounts · identity.read')
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
    expect(
      summaryOf(event('container.stopped', { engine: 'docker', id: 'b2', name: 'shop-web-1' }), t),
    ).toBe('docker · shop-web-1')
    expect(
      summaryOf(
        event('container.removed', {
          engine: 'docker',
          id: 'b2',
          name: 'shop-web-1',
          force: true,
          remove_volumes: false,
        }),
        t,
      ),
    ).toBe('docker · shop-web-1 · audit.summary.killedFirst')
    expect(
      summaryOf(
        event('container.image.removed', {
          engine: 'docker',
          id: 'sha256:bb',
          image: 'redis:7',
          untagged: ['redis:7'],
          deleted: ['sha256:bb'],
          force: true,
        }),
        t,
      ),
    ).toBe('docker · redis:7 · audit.summary.forced')
    expect(
      summaryOf(
        event('container.engine.pruned', {
          engine: 'docker',
          removed: ['container cache', 'image sha256:cc'],
          kept: 1,
          reclaimed_bytes: 1024,
        }),
        t,
      ),
    ).toBe('docker · audit.summary.pruned{"count":2} · audit.summary.kept{"count":1}')
    expect(
      summaryOf(
        event('container.compose.up', {
          engine: 'docker',
          project: 'shop',
          changed: 1,
          failed: ['shop-worker-1'],
        }),
        t,
      ),
    ).toBe('docker · shop · audit.summary.failed{"count":1}')
    expect(
      summaryOf(
        event('container.operation.refused', {
          engine: 'docker',
          operation: 'image.remove',
          code: 'PRECONDITION_FAILED',
          message: 'nats:2.15 is used by the panel',
          container: '',
          image: 'nats:2.15',
        }),
        t,
      ),
    ).toBe('image.remove docker · nats:2.15 · PRECONDITION_FAILED · nats:2.15 is used by the panel')
  })

  it('summarizes roles, token rotations and certificates', () => {
    expect(
      summaryOf(
        event('identity.role.created', {
          role: 'deployer',
          permissions: ['config.read', 'config.apply'],
        }),
        t,
      ),
    ).toBe('deployer · config.read, config.apply')
    expect(
      summaryOf(
        event('identity.token.rotated', {
          token: '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a71',
          replaces: '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a70',
        }),
        t,
      ),
    ).toBe('0190a1b2 → 0190a1b2')
    expect(
      summaryOf(
        event('tls.certificate.created', {
          id: 'example.com',
          names: ['example.com', '*.example.com'],
          source: 'uploaded',
        }),
        t,
      ),
    ).toBe('example.com · example.com, *.example.com · uploaded')
    expect(
      summaryOf(
        event('tls.certificate.refused', {
          id: 'example.com',
          operation: 'replace',
          code: 'VALIDATION_FAILED',
          message: 'the certificate expired',
        }),
        t,
      ),
    ).toBe('replace example.com · VALIDATION_FAILED · the certificate expired')
    expect(toneOf('tls.certificate.refused')).toBe('negative')
  })

  it('summarizes ACME changes, failed issuance and expiring certificates', () => {
    expect(
      summaryOf(
        event('tls.acme.account.created', {
          id: 'letsencrypt',
          directory: 'https://acme-v02.api.letsencrypt.org/directory',
        }),
        t,
      ),
    ).toBe('letsencrypt · acme-v02.api.letsencrypt.org')
    expect(
      summaryOf(
        event('tls.acme.certificate.failed', {
          id: 'shop.example',
          code: 'VALIDATION_FAILED',
          message: 'the CA refused (connection): no answer',
        }),
        t,
      ),
    ).toBe('shop.example · VALIDATION_FAILED · the CA refused (connection): no answer')
    expect(
      summaryOf(
        event('tls.certificate.expiring', { id: 'shop.example', within_days: 7, expired: false }),
        t,
      ),
    ).toBe('shop.example · audit.summary.expiresWithin{"count":7}')
    expect(
      summaryOf(event('tls.certificate.expiring', { id: 'old.example', expired: true }), t),
    ).toBe('old.example · audit.summary.expired')
    expect(toneOf('tls.acme.certificate.failed')).toBe('negative')
    expect(toneOf('tls.certificate.expiring')).toBe('warning')
    expect(toneOf('tls.acme.certificate.created')).toBe('positive')
  })
})
