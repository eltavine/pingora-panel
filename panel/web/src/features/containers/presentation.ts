import type {
  ComposeLogLineView,
  ComposeProjectView,
  ContainerEngineView,
  ContainerLogLineView,
  ContainerLogStreamName,
  ContainerStateName,
  ContainerStatsView,
  ImageLayerView,
  ImageView,
  PortMappingView,
} from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'
import { websocketUrl, type ApiFailure, type ApiLocation } from '@/lib/api'

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

/** A container's labels by name. */
export function sortedLabels(labels: Record<string, string>): { name: string; value: string }[] {
  return Object.entries(labels)
    .map(([name, value]) => ({ name, value }))
    .sort((left, right) => left.name.localeCompare(right.name))
}

/** Whether a container has something running to stop, restart or kill. */
export function stoppable(state: ContainerStateName): boolean {
  return state === 'running' || state === 'restarting' || state === 'paused'
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

/** How many of a container's last lines to read, to choose from; the API reads at most 5000. */
export const LOG_LINE_COUNTS = [100, 200, 500, 1000, 5000] as const

/** How many lines the logs keep while following; the oldest drop off. */
export const LOG_LIMIT = 5_000

const ESCAPE = String.fromCharCode(27)
const BELL = String.fromCharCode(7)
/** Terminal control sequences (ECMA-48): CSI such as colours, OSC such as titles, and others. */
const CONTROL = new RegExp(
  `${ESCAPE}\\[[0-?]*[ -/]*[@-~]|${ESCAPE}\\][^${BELL}${ESCAPE}]*(?:${BELL}|${ESCAPE}\\\\)|${ESCAPE}[@-Z\\\\-_]`,
  'g',
)

/** A line as it reads without its colours and other terminal control sequences. */
export function plainText(text: string): string {
  return text.replace(CONTROL, '')
}

/** Whether a line is from `stream`, or any when it is `all`, and contains `search`, ignoring case. */
export function matchesLine(
  line: ContainerLogLineView,
  stream: ContainerLogStreamName | 'all',
  search: string,
): boolean {
  const wanted = search.trim().toLowerCase()
  return (
    (stream === 'all' || line.stream === stream) &&
    (!wanted || plainText(line.text).toLowerCase().includes(wanted))
  )
}

/** Lines as a log file: each with the time its engine recorded and its text as printed. */
export function logFile(lines: readonly ContainerLogLineView[]): string {
  return lines.map((line) => `${line.time} ${line.text}\n`).join('')
}

/** Where a container's lines are followed, with the WebSocket scheme. */
export function logTailUrl(
  engine: string,
  container: string,
  query: Record<string, string>,
  at: ApiLocation,
): string {
  const path = `/api/v1/container-engines/${encodeURIComponent(engine)}/containers/${encodeURIComponent(container)}/logs/tail`
  return websocketUrl(path, query, at)
}

/** The share of its memory limit a container uses; none without a limit. */
export function memoryShare(stats: ContainerStatsView): number | undefined {
  return stats.memory_limit_bytes > 0 ? stats.memory_bytes / stats.memory_limit_bytes : undefined
}

/** An image's ID as `docker images` shortens it: twelve digits without `sha256:`. */
export function shortId(id: string): string {
  return id.replace(/^sha256:/, '').slice(0, 12)
}

/** An image by its first tag, or its short ID when nothing names it. */
export function imageName(image: Pick<ImageView, 'id' | 'tags'>): string {
  return image.tags[0] ?? shortId(image.id)
}

export type ProjectCondition = 'running' | 'partial' | 'stopped'

/** Whether all, some or none of a project's containers run. */
export function projectCondition(
  project: Pick<ComposeProjectView, 'running' | 'containers'>,
): ProjectCondition {
  if (project.running === 0) {
    return 'stopped'
  }
  return project.running < project.containers ? 'partial' : 'running'
}

export function projectTone(condition: ProjectCondition): StatusTone {
  switch (condition) {
    case 'running':
      return 'positive'
    case 'partial':
      return 'warning'
    default:
      return 'neutral'
  }
}

/** Whether a project's line is from `service`, or any when it is empty, and matches as {@link matchesLine} does. */
export function matchesProjectLine(
  line: ComposeLogLineView,
  service: string,
  stream: ContainerLogStreamName | 'all',
  search: string,
): boolean {
  return (!service || line.service === service) && matchesLine(line.line, stream, search)
}

/** A project's lines as a log file, each after its container's name as `docker compose logs` writes them. */
export function projectLogFile(lines: readonly ComposeLogLineView[]): string {
  return lines.map(({ container, line }) => `${line.time} ${container} | ${line.text}\n`).join('')
}

/** How long a pull may take: the host agent's limit, and a margin. */
export const PULL_TIMEOUT_MS = 35 * 60_000

/**
 * How far a pull got, from 0 to 1, by the layers whose size is known:
 * downloading is the first half of a layer and extracting the second, and
 * what the engine had already does not count. Unknown until a size is.
 */
export function pullShare(layers: readonly ImageLayerView[]): number | undefined {
  let done = 0
  let total = 0
  for (const layer of layers) {
    if (layer.state === 'exists' || layer.total_bytes === 0) {
      continue
    }
    const size = layer.total_bytes
    const current = Math.min(layer.current_bytes, size)
    total += 2 * size
    switch (layer.state) {
      case 'downloading':
        done += current
        break
      case 'downloaded':
        done += size
        break
      case 'extracting':
        done += size + current
        break
      case 'complete':
        done += 2 * size
        break
    }
  }
  return total > 0 ? done / total : undefined
}

/** A layer once its image is pulled: complete, unless the engine had it. */
export function finished(layer: ImageLayerView): ImageLayerView {
  return layer.state === 'exists'
    ? layer
    : { ...layer, state: 'complete', current_bytes: layer.total_bytes }
}

/** The layers there is nothing more to do for. */
export function layersDone(layers: readonly ImageLayerView[]): number {
  return layers.filter((layer) => layer.state === 'complete' || layer.state === 'exists').length
}
