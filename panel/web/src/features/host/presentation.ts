import type {
  AgentStatusName,
  CapabilityStateName,
  ContainerStateName,
  DirectoryUsageView,
  DiskLevel,
  HostSummaryView,
  PortListenerView,
} from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'
import { stateTone } from '@/features/containers/presentation'

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

/** Container health checks the console names in its own words. */
const GATEWAY_HEALTH = ['healthy', 'unhealthy', 'starting'] as const

export type GatewayHealthKey = (typeof GATEWAY_HEALTH)[number]

/** The message key for a container's health, or `null` without a check or for one it does not know. */
export function gatewayHealthKey(health: string | null | undefined): GatewayHealthKey | null {
  return health && (GATEWAY_HEALTH as readonly string[]).includes(health)
    ? (health as GatewayHealthKey)
    : null
}

/** The gateway container's tone: its state's, unless a running container's health check disagrees. */
export function gatewayTone(
  state: ContainerStateName,
  health: string | null | undefined,
): StatusTone {
  if (state === 'running' && health === 'unhealthy') {
    return 'negative'
  }
  if (state === 'running' && health === 'starting') {
    return 'pending'
  }
  return stateTone(state)
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
