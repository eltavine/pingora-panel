import { computed, toValue, type MaybeRefOrGetter } from 'vue'
import { useQueries, useQuery } from '@tanstack/vue-query'
import type {
  ContainerEndpointView,
  ContainerView,
  EndpointRouteName,
  SiteLinkView,
} from '@/api/generated'
import {
  listContainersOptions,
  listEnginesOptions,
  siteLinksOptions,
} from '@/api/generated/@tanstack/vue-query.gen'

/** An endpoint as an upstream node names it, such as `127.0.0.1:8081` or `[fd00::2]:80`. */
export function endpointAddress(endpoint: Pick<ContainerEndpointView, 'host' | 'port'>): string {
  return endpoint.host.includes(':')
    ? `[${endpoint.host}]:${endpoint.port}`
    : `${endpoint.host}:${endpoint.port}`
}

/**
 * The endpoint a site for a container proxies to unless another is chosen:
 * the first for the port its labels declare, else its first, which is a
 * published port when it has one.
 */
export function defaultEndpoint(
  container: Pick<ContainerView, 'endpoints' | 'declared_site'>,
): ContainerEndpointView | undefined {
  const declared = container.declared_site?.port
  const forDeclared =
    declared === undefined || declared === null
      ? undefined
      : container.endpoints.find((endpoint) => endpoint.container_port === declared)
  return forDeclared ?? container.endpoints[0]
}

/** The links of each container, by its ID. */
export function linksByContainer(links: readonly SiteLinkView[]): Map<string, SiteLinkView[]> {
  const grouped = new Map<string, SiteLinkView[]>()
  for (const link of links) {
    grouped.set(link.container_id, [...(grouped.get(link.container_id) ?? []), link])
  }
  return grouped
}

/** The sites in `links`, each once, in order. */
export function linkedSites(links: readonly SiteLinkView[]): { id: string; name: string }[] {
  const sites = new Map<string, string>()
  for (const link of links) {
    sites.set(link.site_id, link.site)
  }
  return [...sites].map(([id, name]) => ({ id, name }))
}

/** The engines that are enabled and answer, read only while `enabled`. */
function useReachableEngines(enabled: MaybeRefOrGetter<boolean>) {
  const engines = useQuery(
    computed(() => ({ ...listEnginesOptions(), enabled: toValue(enabled), retry: false })),
  )
  return computed(() =>
    (engines.data.value?.engines ?? [])
      .filter((engine) => engine.enabled && engine.reachable)
      .map((engine) => engine.id),
  )
}

/** An endpoint of a running container, to point an upstream's node at. */
export interface DiscoveredEndpoint {
  engine: string
  container: string
  host: string
  port: number
  containerPort: number
  route: EndpointRouteName
  network?: string | null
}

/** The endpoints of the running containers on every engine that answers, published first. */
export function useContainerEndpoints(enabled: MaybeRefOrGetter<boolean>) {
  const engines = useReachableEngines(enabled)
  const lists = useQueries({
    queries: computed(() =>
      engines.value.map((engine) => ({
        ...listContainersOptions({ path: { engine }, query: { state: 'running' } }),
        enabled: toValue(enabled),
      })),
    ),
  })
  return computed((): DiscoveredEndpoint[] =>
    lists.value.flatMap((list, index) =>
      (list.data?.containers ?? []).flatMap((container) =>
        container.endpoints.map((endpoint) => ({
          engine: engines.value[index] ?? '',
          container: container.names[0] ?? container.id.slice(0, 12),
          host: endpoint.host,
          port: endpoint.port,
          containerPort: endpoint.container_port,
          route: endpoint.route,
          network: endpoint.network,
        })),
      ),
    ),
  )
}

/** A container a site's upstreams point at, on the engine it runs on. */
export interface LinkedContainer extends SiteLinkView {
  engine: string
}

/** The containers on every engine that answers that the site `siteId` points at. */
export function useSiteContainers(
  siteId: MaybeRefOrGetter<string>,
  enabled: MaybeRefOrGetter<boolean>,
) {
  const engines = useReachableEngines(enabled)
  const links = useQueries({
    queries: computed(() =>
      engines.value.map((engine) => ({
        ...siteLinksOptions({ path: { engine } }),
        enabled: toValue(enabled),
      })),
    ),
  })
  return computed((): LinkedContainer[] =>
    links.value.flatMap((found, index) =>
      (found.data?.links ?? [])
        .filter((link) => link.site_id === toValue(siteId))
        .map((link) => ({ ...link, engine: engines.value[index] ?? '' })),
    ),
  )
}
