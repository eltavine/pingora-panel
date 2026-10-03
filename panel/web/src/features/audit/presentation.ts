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
] as const

export type KnownType = (typeof KNOWN_TYPES)[number]

export function isKnownType(type: string): type is KnownType {
  return (KNOWN_TYPES as readonly string[]).includes(type)
}

/** The message key naming an event type; message keys cannot hold dots. */
export function typeKey(type: KnownType): string {
  return `audit.types.${type.replace(/\./g, '_')}`
}

/** Refusals and failures read as negative; everything else happened. */
export function toneOf(type: string): StatusTone {
  return /\.(refused|failed|rejected)$/.test(type) ? 'negative' : 'positive'
}

type Translate = (key: string, values?: Record<string, unknown>) => string

function text(value: unknown): string {
  return value === null || value === undefined ? '' : String(value)
}

function list(value: unknown): string {
  return Array.isArray(value) ? value.map(text).join(', ') : text(value)
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
      return t('audit.summary.generation', {
        generation: text(data.generation),
        workers: text(data.workers),
      })
    case 'gateway.endpoint.drained':
    case 'gateway.endpoint.restored':
      return text(data.endpoint)
    default:
      return JSON.stringify(data).slice(0, 120)
  }
}
