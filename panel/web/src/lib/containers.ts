import type { ContainerStateName } from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'

const ENGINE_NAMES: Record<string, string> = { docker: 'Docker', podman: 'Podman' }

/** An engine's product name, or its identifier for one the console does not know. */
export function engineName(id: string): string {
  return ENGINE_NAMES[id] ?? id
}

export function stateTone(state: ContainerStateName): StatusTone {
  switch (state) {
    case 'running':
      return 'positive'
    case 'restarting':
    case 'removing':
    case 'stopping':
      return 'pending'
    case 'paused':
      return 'warning'
    case 'dead':
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
