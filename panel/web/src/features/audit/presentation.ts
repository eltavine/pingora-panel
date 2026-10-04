import type { AuditEvent } from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'

/** Event types the console names; others show their type as recorded. */
export const KNOWN_TYPES = [
  'config.draft.changed',
  'config.change.refused',
  'config.draft.applied',
  'config.apply.checked',
  'config.apply.rejected',
  'config.apply.failed',
  'config.revision.noted',
  'gateway.snapshot.prepared',
  'gateway.snapshot.activated',
  'gateway.snapshot.aborted',
  'gateway.snapshot.refused',
  'gateway.reloaded',
  'gateway.workers.changed',
  'gateway.shutdown.requested',
  'gateway.endpoint.drained',
  'gateway.endpoint.restored',
  'gateway.operation.refused',
  'gateway.logs.deleted',
  'identity.account.created',
  'identity.account.updated',
  'identity.password.changed',
  'identity.login.succeeded',
  'identity.login.failed',
  'identity.session.ended',
  'identity.token.created',
  'identity.token.revoked',
  'identity.token.rotated',
  'identity.role.created',
  'identity.role.updated',
  'identity.role.deleted',
  'identity.access.denied',
  'tls.certificate.created',
  'tls.certificate.replaced',
  'tls.certificate.deleted',
  'tls.certificate.refused',
  'tls.certificate.expiring',
  'tls.acme.account.created',
  'tls.acme.account.deleted',
  'tls.acme.account.refused',
  'tls.acme.certificate.created',
  'tls.acme.certificate.renewal_requested',
  'tls.acme.certificate.failed',
  'tls.acme.certificate.deleted',
  'tls.acme.certificate.refused',
  'tls.acme.dns_provider.created',
  'tls.acme.dns_provider.updated',
  'tls.acme.dns_provider.deleted',
  'tls.acme.dns_provider.refused',
] as const

export type KnownType = (typeof KNOWN_TYPES)[number]

export function isKnownType(type: string): type is KnownType {
  return (KNOWN_TYPES as readonly string[]).includes(type)
}

/** The message key naming an event type; message keys cannot hold dots. */
export function typeKey(type: KnownType): string {
  return `audit.types.${type.replace(/\./g, '_')}`
}

/** Refusals and failures read as negative and warnings as such; everything
 * else happened. */
export function toneOf(type: string): StatusTone {
  if (/\.(refused|failed|rejected|denied)$/.test(type)) {
    return 'negative'
  }
  return type.endsWith('.expiring') ? 'warning' : 'positive'
}

type Translate = (key: string, values?: Record<string, unknown>, plural?: number) => string

function text(value: unknown): string {
  return value === null || value === undefined ? '' : String(value)
}

function list(value: unknown): string {
  return Array.isArray(value) ? value.map(text).join(', ') : text(value)
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function host(value: unknown): string {
  try {
    return new URL(text(value)).host
  } catch {
    return text(value)
  }
}

/** A one-line account of what happened, from the event's data. */
export function summaryOf(event: AuditEvent, t: Translate): string {
  const data = (event.data ?? {}) as Record<string, unknown>
  const note = data.note ? ` · ${text(data.note)}` : ''
  switch (event.event_type) {
    case 'config.draft.changed':
      return `${text(data.operation)} ${text(data.resource)} → v${text(data.version)}`
    case 'config.change.refused':
      return `${text(data.operation)} ${text(data.resource)} · ${text(data.code)}`
    case 'config.draft.applied':
      return `v${text(data.version)} → #${text(data.revision)}${note}`
    case 'config.apply.checked':
    case 'config.apply.rejected':
      return data.valid ? `v${text(data.version)}` : `v${text(data.version)} · ${list(data.codes)}`
    case 'config.apply.failed':
    case 'gateway.snapshot.refused':
    case 'gateway.operation.refused':
      return [data.operation, data.code, data.message].map(text).filter(Boolean).join(' · ')
    case 'config.revision.noted':
      return `#${text(data.revision)}${note}`
    case 'gateway.snapshot.prepared':
    case 'gateway.snapshot.activated':
      return `#${text(data.revision_id)} · ${text(data.content_hash).slice(0, 12)}`
    case 'gateway.reloaded':
    case 'gateway.workers.changed':
      return t(
        'audit.summary.generation',
        { generation: text(data.generation), workers: text(data.workers) },
        Number(data.workers),
      )
    case 'gateway.endpoint.drained':
    case 'gateway.endpoint.restored':
      return text(data.endpoint)
    case 'gateway.logs.deleted':
      return text(data.site) || t('audit.summary.everySite')
    case 'identity.account.created':
      return `${text(data.username)} · ${list(data.roles)}`
    case 'identity.account.updated':
      return [
        data.disabled === true && t('audit.summary.disabled'),
        data.disabled === false && t('audit.summary.enabled'),
        data.unlocked === true && t('audit.summary.unlocked'),
        isRecord(data.roles) && list(data.roles.names),
      ]
        .filter(Boolean)
        .join(' · ')
    case 'identity.login.succeeded':
    case 'identity.login.failed': {
      const attempt = (data.attempt ?? {}) as Record<string, unknown>
      return [attempt.username, attempt.client_address, data.reason ?? data.transport]
        .map(text)
        .filter(Boolean)
        .join(' · ')
    }
    case 'identity.session.ended':
      return text(data.reason)
    case 'identity.token.created':
      return `${text(data.name)} · ${list(data.permissions)}`
    case 'identity.token.rotated':
      return `${text(data.replaces).slice(0, 8)} → ${text(data.token).slice(0, 8)}`
    case 'identity.role.created':
    case 'identity.role.updated':
      return `${text(data.role)} · ${list(data.permissions)}`
    case 'identity.role.deleted':
      return text(data.role)
    case 'tls.certificate.created':
      return `${text(data.id)} · ${list(data.names)} · ${text(data.source)}`
    case 'tls.certificate.replaced':
      return `${text(data.id)} · v${text(data.version)} · ${list(data.names)}`
    case 'tls.acme.dns_provider.created':
    case 'tls.acme.dns_provider.updated':
      return `${text(data.id)} · ${text(data.server)} · ${list(data.zones)}`
    case 'tls.certificate.deleted':
    case 'tls.acme.dns_provider.deleted':
    case 'tls.acme.account.deleted':
    case 'tls.acme.certificate.deleted':
    case 'tls.acme.certificate.renewal_requested':
      return text(data.id)
    case 'tls.certificate.refused':
    case 'tls.acme.account.refused':
    case 'tls.acme.certificate.refused':
    case 'tls.acme.dns_provider.refused':
      return [`${text(data.operation)} ${text(data.id)}`, data.code, data.message]
        .map(text)
        .filter(Boolean)
        .join(' · ')
    case 'tls.certificate.expiring':
      return `${text(data.id)} · ${
        data.expired
          ? t('audit.summary.expired')
          : t(
              'audit.summary.expiresWithin',
              { count: Number(data.within_days) },
              Number(data.within_days),
            )
      }`
    case 'tls.acme.account.created':
      return `${text(data.id)} · ${host(data.directory)}`
    case 'tls.acme.certificate.created':
      return `${text(data.id)} · ${list(data.names)} · ${text(data.challenge)}`
    case 'tls.acme.certificate.failed':
      return [data.id, data.code, data.message].map(text).filter(Boolean).join(' · ')
    case 'identity.access.denied':
      return [`${text(data.method)} ${text(data.route)}`, data.permission ?? data.reason]
        .map(text)
        .filter(Boolean)
        .join(' · ')
    default:
      return JSON.stringify(data).slice(0, 120)
  }
}
