import type { ReadinessState } from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'

/** How the system page reads versions and readiness again. */
export const REFRESH_INTERVAL_MS = 60_000

export function readinessTone(state: ReadinessState): StatusTone {
  switch (state) {
    case 'pass':
      return 'positive'
    case 'warn':
      return 'warning'
    case 'fail':
      return 'negative'
    default:
      return 'neutral'
  }
}

/** The file the bundle is saved as, named after when it was put together. */
export function bundleName(generatedAt: string): string {
  const stamp = generatedAt.replace(/\.\d+/, '').replace(/[-:]/g, '')
  return `pingora-panel-diagnostics-${stamp}.json`
}

/** An image digest shortened to its algorithm and first twelve digits. */
export function shortDigest(digest: string): string {
  const [algorithm, hex] = digest.split(':')
  return hex ? `${algorithm}:${hex.slice(0, 12)}` : digest
}

const ENGINES: Record<string, string> = { docker: 'Docker', podman: 'Podman' }

export function engineName(engine: string): string {
  return ENGINES[engine] ?? engine
}
