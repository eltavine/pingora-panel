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
  'observability.alert_rule.created',
  'observability.alert_rule.updated',
  'observability.alert_rule.deleted',
  'observability.alert_rule.refused',
  'observability.alert_channel.created',
  'observability.alert_channel.rotated',
  'observability.alert_channel.deleted',
  'observability.alert_channel.refused',
  'observability.alert.fired',
  'observability.alert.resolved',
  'host.gateway_service.started',
  'host.gateway_service.stopped',
  'host.gateway_service.restarted',
  'host.operation.refused',
  'files.file.written',
  'files.directory.created',
  'files.entry.removed',
  'files.operation.refused',
  'container.engine.enabled',
  'container.engine.disabled',
  'container.started',
  'container.stopped',
  'container.restarted',
  'container.killed',
  'container.removed',
  'container.image.removed',
  'container.image.pulled',
  'container.engine.pruned',
  'container.compose.up',
  'container.compose.down',
  'container.compose.restarted',
  'container.operation.refused',
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
  return /\.(expiring|fired)$/.test(type) ? 'warning' : 'positive'
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
    case 'host.operation.refused':
      return [data.operation, data.code, data.message].map(text).filter(Boolean).join(' · ')
    case 'files.file.written':
      return [
        text(data.path),
        data.created === true && t('audit.summary.created'),
        typeof data.sha256 === 'string' && data.sha256.slice(0, 12),
      ]
        .filter(Boolean)
        .join(' · ')
    case 'files.directory.created':
      return text(data.path)
    case 'files.entry.removed': {
      const removed = Number(data.removed ?? 1)
      return [
        text(data.path),
        removed > 1 && t('audit.summary.entries', { count: removed }, removed),
      ]
        .filter(Boolean)
        .join(' · ')
    }
    case 'files.operation.refused':
      return [`${text(data.operation)} ${text(data.path)}`, data.code, data.message]
        .map(text)
        .filter(Boolean)
        .join(' · ')
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
    case 'observability.alert_rule.refused':
    case 'observability.alert_channel.refused':
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
    case 'observability.alert_rule.created':
    case 'observability.alert_rule.updated': {
      const settings = (data.settings ?? {}) as Record<string, unknown>
      return `${text(data.id)} · ${text(settings.measure)} ${text(settings.comparison)} ${text(settings.threshold)}`
    }
    case 'observability.alert_channel.created':
    case 'observability.alert_channel.rotated':
      return `${text(data.id)} · ${text(data.target)}`
    case 'observability.alert_rule.deleted':
    case 'observability.alert_channel.deleted':
      return text(data.id)
    case 'observability.alert.fired':
      return `${text(data.name)} · ${text(data.value)}`
    case 'observability.alert.resolved':
      return text(data.name)
    case 'host.gateway_service.started':
    case 'host.gateway_service.stopped':
    case 'host.gateway_service.restarted':
      return `${text(data.container)} · ${text(data.state)}`
    case 'container.engine.enabled':
    case 'container.engine.disabled':
      return text(data.engine)
    case 'container.started':
    case 'container.stopped':
    case 'container.restarted':
    case 'container.killed':
      return `${text(data.engine)} · ${text(data.name)}`
    case 'container.removed':
      return [
        `${text(data.engine)} · ${text(data.name)}`,
        data.force === true && t('audit.summary.killedFirst'),
        data.remove_volumes === true && t('audit.summary.volumesRemoved'),
      ]
        .filter(Boolean)
        .join(' · ')
    case 'container.engine.pruned': {
      const removed = Array.isArray(data.removed) ? data.removed.length : 0
      const kept = Number(data.kept) || 0
      return [
        text(data.engine),
        t('audit.summary.pruned', { count: removed }, removed),
        kept > 0 && t('audit.summary.kept', { count: kept }, kept),
      ]
        .filter(Boolean)
        .join(' · ')
    }
    case 'container.compose.up':
    case 'container.compose.down':
    case 'container.compose.restarted': {
      const failed = Array.isArray(data.failed) ? data.failed.length : 0
      return [
        `${text(data.engine)} · ${text(data.project)}`,
        failed > 0 && t('audit.summary.failed', { count: failed }, failed),
      ]
        .filter(Boolean)
        .join(' · ')
    }
    case 'container.image.removed':
      return [
        `${text(data.engine)} · ${text(data.image)}`,
        data.force === true && t('audit.summary.forced'),
      ]
        .filter(Boolean)
        .join(' · ')
    case 'container.image.pulled':
      return [
        `${text(data.engine)} · ${text(data.reference)}`,
        data.updated !== true && t('audit.summary.upToDate'),
      ]
        .filter(Boolean)
        .join(' · ')
    case 'container.operation.refused':
      return [
        `${text(data.operation)} ${text(data.engine)}`,
        data.container || data.image || data.project,
        data.code,
        data.message,
      ]
        .map(text)
        .filter(Boolean)
        .join(' · ')
    case 'identity.access.denied':
      return [`${text(data.method)} ${text(data.route)}`, data.permission ?? data.reason]
        .map(text)
        .filter(Boolean)
        .join(' · ')
    default:
      return JSON.stringify(data).slice(0, 120)
  }
}
