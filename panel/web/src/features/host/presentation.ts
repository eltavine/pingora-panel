import type {
  AgentStatusName,
  CapabilityStateName,
  DirectoryUsageView,
  DiskLevel,
  HostSummaryView,
  PortListenerView,
} from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'

/** How the host page reads figures again. */
export const REFRESH_INTERVAL_MS = 30_000
/** Directory sizes take the agent a walk of the disk, so they are read less often. */
export const DIRECTORY_REFRESH_MS = 300_000

export function agentTone(status: AgentStatusName): StatusTone {
  switch (status) {
    case 'connected':
      return 'positive'
    case 'unreachable':
      return 'negative'
    default:
      return 'neutral'
  }
}

export function capabilityTone(state: CapabilityStateName): StatusTone {
  switch (state) {
    case 'available':
      return 'positive'
    case 'denied':
      return 'warning'
    case 'unreachable':
      return 'negative'
    default:
      return 'neutral'
  }
}

/** systemd's active states the console names in its own words. */
const UNIT_STATES = [
  'active',
  'inactive',
  'failed',
  'activating',
  'deactivating',
  'reloading',
] as const

export type UnitStateKey = (typeof UNIT_STATES)[number]

/** The message key for a systemd active state, or `null` for one to show as systemd says it. */
export function unitStateKey(state: string): UnitStateKey | null {
  return (UNIT_STATES as readonly string[]).includes(state) ? (state as UnitStateKey) : null
}

export function unitTone(state: string): StatusTone {
  switch (state) {
    case 'active':
      return 'positive'
    case 'failed':
      return 'negative'
    case 'activating':
    case 'deactivating':
    case 'reloading':
      return 'pending'
    default:
      return 'neutral'
  }
}

/** The gateway's process name, as the host agent reports it. */
const GATEWAY_PROCESS = 'gatewayd'

export type PortHolder = 'gateway' | 'other' | 'unknown'

/** Whether the gateway holds a listening socket, another process does, or the agent cannot see. */
export function portHolder(listener: PortListenerView): PortHolder {
  if (listener.processes.length === 0) {
    return 'unknown'
  }
  return listener.processes.every((process) => process.name === GATEWAY_PROCESS)
    ? 'gateway'
    : 'other'
}

export function holderTone(holder: PortHolder): StatusTone {
  switch (holder) {
    case 'gateway':
      return 'positive'
    case 'other':
      return 'warning'
    default:
      return 'neutral'
  }
}

export type DirectoryNote =
  { kind: 'missing' } | { kind: 'partial' } | { kind: 'unreadable'; n: number }

/** What qualifies a directory's figures, if anything. */
export function directoryNotes(directory: DirectoryUsageView): DirectoryNote[] {
  if (!directory.present) {
    return [{ kind: 'missing' }]
  }
  const notes: DirectoryNote[] = []
  if (directory.truncated) {
    notes.push({ kind: 'partial' })
  }
  if (directory.unreadable > 0) {
    notes.push({ kind: 'unreadable', n: directory.unreadable })
  }
  return notes
}

export function levelTone(level: DiskLevel): StatusTone {
  switch (level) {
    case 'critical':
      return 'negative'
    case 'warning':
      return 'warning'
    default:
      return 'positive'
  }
}

/** Uptime in its largest two units, such as 3 days and 4 hours. */
export function uptimeParts(seconds: number): { unit: 'days' | 'hours' | 'minutes'; n: number }[] {
  const days = Math.floor(seconds / 86_400)
  const hours = Math.floor((seconds % 86_400) / 3_600)
  const minutes = Math.floor((seconds % 3_600) / 60)
  if (days > 0) {
    return hours > 0
      ? [
          { unit: 'days', n: days },
          { unit: 'hours', n: hours },
        ]
      : [{ unit: 'days', n: days }]
  }
  if (hours > 0) {
    return [
      { unit: 'hours', n: hours },
      { unit: 'minutes', n: minutes },
    ]
  }
  return [{ unit: 'minutes', n: minutes }]
}

/** The share of memory in use, from 0 to 1. */
export function memoryUsed(host: HostSummaryView): number {
  return host.memory_total_bytes > 0 ? 1 - host.memory_available_bytes / host.memory_total_bytes : 0
}
