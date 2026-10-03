import type {
  EndpointHealthResponse,
  HealthCheckProtocol,
  LoadBalancingPolicy,
  NodeInput,
  UpstreamInput,
  UpstreamNode,
  UpstreamView,
} from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'
import { optionalNumber, optionalText } from '@/features/sites/forms'

export type Algorithm = 'round_robin' | 'random' | 'consistent_hash'
export const ALGORITHMS: readonly Algorithm[] = ['round_robin', 'random', 'consistent_hash']
export const HEALTH_PROTOCOLS: readonly HealthCheckProtocol[] = ['http', 'tcp']
export const HEALTH_METHODS = ['GET', 'HEAD'] as const

type NumberInput = number | string

export interface UpstreamForm {
  name: string
  algorithm: Algorithm
  hashKey: string
  hostHeader: string
  note: string
  nodes: string
  verifyCertificate: boolean
  verifyHostname: boolean
  sni: string
  caSecretId: string
  connectTimeout: NumberInput
  readTimeout: NumberInput
  writeTimeout: NumberInput
  idleTimeout: NumberInput
  keepalive: boolean
  maxConnections: NumberInput
  http2: boolean
  healthEnabled: boolean
  healthProtocol: HealthCheckProtocol
  healthPath: string
  healthMethod: string
  healthHost: string
  healthInterval: NumberInput
  healthTimeout: NumberInput
  healthyThreshold: NumberInput
  unhealthyThreshold: NumberInput
  expectedStatuses: string
  passiveEnabled: boolean
  failureThreshold: NumberInput
  ejection: NumberInput
}

export interface NodeForm {
  host: string
  port: NumberInput
  weight: NumberInput
  enabled: boolean
  backup: boolean
  tls: boolean
  sni: string
  unixSocket: string
  note: string
}

export function algorithmOf(policy: LoadBalancingPolicy | undefined): Algorithm {
  if (policy === undefined || typeof policy === 'string') {
    return policy ?? 'round_robin'
  }
  return 'consistent_hash'
}

function policyOf(form: UpstreamForm): LoadBalancingPolicy {
  return form.algorithm === 'consistent_hash'
    ? { consistent_hash: { key: form.hashKey.trim() } }
    : form.algorithm
}

export function upstreamForm(upstream?: UpstreamView): UpstreamForm {
  const balancing = upstream?.balancing
  const connection = upstream?.connection
  const tls = upstream?.tls
  const health = upstream?.health_check
  const passive = upstream?.passive_health
  return {
    name: upstream?.name ?? '',
    algorithm: algorithmOf(balancing),
    hashKey: typeof balancing === 'object' ? balancing.consistent_hash.key : 'client_ip',
    hostHeader: upstream?.host_header ?? '',
    note: upstream?.note ?? '',
    nodes: '',
    verifyCertificate: tls?.verify_certificate ?? true,
    verifyHostname: tls?.verify_hostname ?? true,
    sni: tls?.sni ?? '',
    caSecretId: tls?.ca_secret_id ?? '',
    connectTimeout: connection?.connect_timeout_ms ?? '',
    readTimeout: connection?.read_timeout_ms ?? '',
    writeTimeout: connection?.write_timeout_ms ?? '',
    idleTimeout: connection?.idle_timeout_ms ?? '',
    keepalive: connection?.keepalive ?? true,
    maxConnections: connection?.max_connections ?? '',
    http2: connection?.http2 ?? false,
    healthEnabled: health != null,
    healthProtocol: health?.protocol ?? 'http',
    healthPath: health?.path ?? '/',
    healthMethod: health?.method ?? 'GET',
    healthHost: health?.host ?? '',
    healthInterval: health?.interval_ms ?? 5000,
    healthTimeout: health?.timeout_ms ?? 1000,
    healthyThreshold: health?.healthy_threshold ?? 2,
    unhealthyThreshold: health?.unhealthy_threshold ?? 3,
    expectedStatuses: (health?.expected_statuses ?? []).join(', '),
    passiveEnabled: passive != null,
    failureThreshold: passive?.failure_threshold ?? 5,
    ejection: passive?.ejection_ms ?? 30000,
  }
}

/**
 * One node per line: `host:port [weight=N] [backup] [tls]`. IPv6 hosts use
 * brackets. Returns the line numbers that do not parse.
 */
export function parseNodes(text: string): { nodes: NodeInput[]; invalid: number[] } {
  const nodes: NodeInput[] = []
  const invalid: number[] = []
  text.split('\n').forEach((raw, index) => {
    const line = raw.split('#')[0]!.trim()
    if (line === '') {
      return
    }
    const [address = '', ...flags] = line.split(/\s+/)
    const match = /^(\[[^\]]+\]|[^:[\]]+):(\d{1,5})$/.exec(address)
    const port = Number(match?.[2])
    if (!match || port < 1 || port > 65535) {
      invalid.push(index + 1)
      return
    }
    const node: NodeInput = { host: match[1]!.replace(/^\[|\]$/g, ''), port, weight: 1 }
    for (const flag of flags) {
      const weight = /^(?:w|weight)=(\d+)$/.exec(flag)
      if (weight) {
        node.weight = Number(weight[1])
      } else if (flag === 'backup') {
        node.backup = true
      } else if (flag === 'tls') {
        node.tls = true
      } else {
        invalid.push(index + 1)
        return
      }
    }
    nodes.push(node)
  })
  return { nodes, invalid }
}

export function nodeInputOf(node: UpstreamNode): NodeInput {
  return { ...node }
}

/** Replacement is total, so editing keeps the nodes as they are. */
export function upstreamInput(form: UpstreamForm, upstream?: UpstreamView): UpstreamInput {
  const statuses = form.expectedStatuses
    .split(',')
    .map((value) => Number(value.trim()))
    .filter((value) => Number.isInteger(value) && value > 0)
  return {
    name: form.name.trim(),
    balancing: policyOf(form),
    host_header: optionalText(form.hostHeader),
    note: optionalText(form.note),
    nodes: upstream ? (upstream.nodes ?? []).map(nodeInputOf) : parseNodes(form.nodes).nodes,
    tls: {
      verify_certificate: form.verifyCertificate,
      verify_hostname: form.verifyHostname,
      sni: optionalText(form.sni),
      ca_secret_id: optionalText(form.caSecretId),
    },
    connection: {
      connect_timeout_ms: optionalNumber(form.connectTimeout),
      read_timeout_ms: optionalNumber(form.readTimeout),
      write_timeout_ms: optionalNumber(form.writeTimeout),
      idle_timeout_ms: optionalNumber(form.idleTimeout),
      keepalive: form.keepalive,
      max_connections: optionalNumber(form.maxConnections),
      http2: form.http2,
    },
    health_check: form.healthEnabled
      ? {
          protocol: form.healthProtocol,
          path: form.healthPath.trim() || '/',
          method: form.healthMethod,
          host: optionalText(form.healthHost),
          interval_ms: optionalNumber(form.healthInterval) ?? 5000,
          timeout_ms: optionalNumber(form.healthTimeout) ?? 1000,
          healthy_threshold: optionalNumber(form.healthyThreshold) ?? 2,
          unhealthy_threshold: optionalNumber(form.unhealthyThreshold) ?? 3,
          expected_statuses: statuses,
        }
      : null,
    passive_health: form.passiveEnabled
      ? {
          failure_threshold: optionalNumber(form.failureThreshold) ?? 5,
          ejection_ms: optionalNumber(form.ejection) ?? 30000,
        }
      : null,
  }
}

export function nodeForm(node?: UpstreamNode): NodeForm {
  return {
    host: node?.host ?? '',
    port: node?.port ?? 80,
    weight: node?.weight ?? 1,
    enabled: node?.enabled ?? true,
    backup: node?.backup ?? false,
    tls: node?.tls ?? false,
    sni: node?.sni ?? '',
    unixSocket: node?.unix_socket ?? '',
    note: node?.note ?? '',
  }
}

export function nodeInput(form: NodeForm, id?: string): NodeInput {
  return {
    id: id ?? null,
    host: form.host.trim(),
    port: optionalNumber(form.port) ?? 0,
    weight: optionalNumber(form.weight) ?? 1,
    enabled: form.enabled,
    backup: form.backup,
    tls: form.tls,
    sni: optionalText(form.sni),
    unix_socket: optionalText(form.unixSocket),
    note: optionalText(form.note),
  }
}

export function nodeAddress(node: { host: string; port: number }): string {
  return node.host.includes(':') ? `[${node.host}]:${node.port}` : `${node.host}:${node.port}`
}

export type NodeState =
  'healthy' | 'unhealthy' | 'unchecked' | 'drained' | 'ejected' | 'disabledNode'

export const nodeTones: Record<NodeState, StatusTone> = {
  healthy: 'positive',
  unhealthy: 'negative',
  unchecked: 'neutral',
  drained: 'warning',
  ejected: 'warning',
  disabledNode: 'neutral',
}

/** The one state an operator acts on first, from most to least specific. */
export function nodeState(
  node: UpstreamNode,
  health: EndpointHealthResponse | undefined,
  now: number = Date.now(),
): NodeState {
  if (!(node.enabled ?? true)) {
    return 'disabledNode'
  }
  if (!health) {
    return 'unchecked'
  }
  if (health.drained) {
    return 'drained'
  }
  if (health.ejected_until && Date.parse(health.ejected_until) > now) {
    return 'ejected'
  }
  return health.healthy ? 'healthy' : 'unhealthy'
}

export function latency(us: number | null | undefined): string {
  return us == null ? '—' : `${(us / 1000).toFixed(us < 10_000 ? 2 : 1)} ms`
}
