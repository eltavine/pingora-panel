import type { DiskLevel, HostSummaryView } from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'

/** How the host page reads figures again. */
export const REFRESH_INTERVAL_MS = 30_000

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
