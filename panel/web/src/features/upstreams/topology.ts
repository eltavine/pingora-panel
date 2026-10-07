import type {
  Action,
  EndpointHealthResponse,
  MatchKind,
  SiteView,
  UpstreamHealthReportResponse,
  UpstreamView,
} from '@/api/generated'
import { nodeAddress, nodeState, type NodeState } from './forms'

/** Names an item of the topology and the element that shows it. */
export type TopologyKey = `${'site' | 'route' | 'pool' | 'node'}:${string}`

export interface TopologySite {
  key: TopologyKey
  id: string
  name: string
  hosts: string[]
  /** The pool the site's own action sends every request no route takes to. */
  pool?: string
}

export interface TopologyRoute {
  key: TopologyKey
  id: string
  site: string
  kind: MatchKind
  path: string
  name?: string
  pool: string
}

export interface TopologyPool {
  key: TopologyKey
  id: string
  name: string
  healthy: number
  total: number
  /** Whether a live site or route sends traffic here. */
  used: boolean
}

export interface TopologyNode {
  key: TopologyKey
  id: string
  pool: string
  address: string
  backup: boolean
  state: NodeState
  health?: EndpointHealthResponse
}

export interface Topology {
  sites: TopologySite[]
  routes: TopologyRoute[]
  pools: TopologyPool[]
  nodes: TopologyNode[]
  /** From the item that sends to the item it reaches. */
  edges: [TopologyKey, TopologyKey][]
}

function proxied(action: Action): string | undefined {
  return action.type === 'proxy' ? action.upstream_id : undefined
}

/**
 * How live sites and their routes reach upstream pools, and the pools'
 * nodes with the health the gateway reports. Sites that send nothing to an
 * upstream are left out; every pool is kept, used or not.
 */
export function topology(
  sites: SiteView[],
  upstreams: UpstreamView[],
  health?: UpstreamHealthReportResponse,
  now: number = Date.now(),
): Topology {
  const pools = new Set(upstreams.map((upstream) => upstream.id))
  const reports = new Map(
    (health?.upstreams ?? []).map((report) => [
      report.upstream_id,
      new Map(report.nodes.map((node) => [node.node_id, node])),
    ]),
  )
  const result: Topology = { sites: [], routes: [], pools: [], nodes: [], edges: [] }

  for (const site of sites) {
    if (!site.enabled || site.deleted_at) {
      continue
    }
    const own = proxied(site.action)
    const pool = own && pools.has(own) ? own : undefined
    const routes = (site.routes ?? []).filter((route) => {
      const target = proxied(route.action)
      return route.enabled && target !== undefined && pools.has(target)
    })
    if (!pool && routes.length === 0) {
      continue
    }
    const key: TopologyKey = `site:${site.id}`
    result.sites.push({
      key,
      id: site.id,
      name: site.name,
      hosts: (site.domains ?? []).map((domain) => site.unicode_hosts?.[domain.host] ?? domain.host),
      pool,
    })
    if (pool) {
      result.edges.push([key, `pool:${pool}`])
    }
    for (const route of routes) {
      const target = proxied(route.action)!
      const routeKey: TopologyKey = `route:${route.id}`
      result.routes.push({
        key: routeKey,
        id: route.id,
        site: site.name,
        kind: route.match.kind,
        path: route.match.path,
        name: route.name ?? undefined,
        pool: target,
      })
      result.edges.push([key, routeKey], [routeKey, `pool:${target}`])
    }
  }

  const reached = new Set(result.edges.map(([, to]) => to))
  for (const upstream of upstreams) {
    const key: TopologyKey = `pool:${upstream.id}`
    const report = reports.get(upstream.id)
    const nodes: TopologyNode[] = (upstream.nodes ?? []).map((node) => {
      const endpoint = report?.get(node.id)
      return {
        key: `node:${upstream.id}/${node.id}`,
        id: node.id,
        pool: upstream.id,
        address: node.unix_socket ? `unix:${node.unix_socket}` : nodeAddress(node),
        backup: node.backup ?? false,
        state: nodeState(node, endpoint, now),
        health: endpoint,
      }
    })
    result.pools.push({
      key,
      id: upstream.id,
      name: upstream.name,
      healthy: nodes.filter((node) => node.state === 'healthy').length,
      total: nodes.length,
      used: reached.has(key),
    })
    for (const node of nodes) {
      result.nodes.push(node)
      result.edges.push([key, node.key])
    }
  }
  return result
}
