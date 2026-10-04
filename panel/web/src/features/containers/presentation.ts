import type { ContainerEngineView, ContainerStateName, PortMappingView } from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'
import type { ApiFailure } from '@/lib/api'

/** How the containers page reads the engines and their containers again. */
export const REFRESH_INTERVAL_MS = 15_000

/** The states the filter offers, in the order a container usually passes through them. */
export const CONTAINER_STATES = [
  'running',
  'paused',
  'restarting',
  'created',
  'exited',
  'dead',
] as const satisfies readonly ContainerStateName[]

const ENGINE_NAMES: Record<string, string> = { docker: 'Docker', podman: 'Podman' }

/** An engine's product name, or its identifier for one the console does not know. */
export function engineName(id: string): string {
  return ENGINE_NAMES[id] ?? id
}

export type EngineCondition = 'reachable' | 'unreachable' | 'disabled'

export function engineCondition(engine: ContainerEngineView): EngineCondition {
  if (!engine.enabled) {
    return 'disabled'
  }
  return engine.reachable ? 'reachable' : 'unreachable'
}

export function conditionTone(condition: EngineCondition): StatusTone {
  switch (condition) {
    case 'reachable':
      return 'positive'
    case 'unreachable':
      return 'negative'
    default:
      return 'neutral'
  }
}

/** The engine the page shows: the one asked for, else the first that answers, else the first. */
export function chosenEngine(
  engines: readonly ContainerEngineView[],
  asked: string,
): string | null {
  if (engines.some((engine) => engine.id === asked)) {
    return asked
  }
  return (engines.find((engine) => engine.enabled && engine.reachable) ?? engines[0])?.id ?? null
}

/** Whether a container has something running to stop, restart or kill. */
export function stoppable(state: ContainerStateName): boolean {
  return state === 'running' || state === 'restarting' || state === 'paused'
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

/** A port as `docker ps` writes it, such as `0.0.0.0:8081->80/tcp`. */
export function portLabel(port: PortMappingView): string {
  const inside = `${port.private_port}/${port.protocol}`
  if (port.public_port === null || port.public_port === undefined) {
    return inside
  }
  const host = port.host_ip.includes(':') ? `[${port.host_ip}]` : port.host_ip
  return `${host}:${port.public_port}->${inside}`
}

/** Whether no host agent manages containers here, which the page explains rather than reports. */
export function withoutAgent(failure: ApiFailure): boolean {
  return failure.kind === 'problem' && failure.problem.code === 'UNSUPPORTED_CAPABILITY'
}
