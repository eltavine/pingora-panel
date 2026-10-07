<script setup lang="ts">
import { computed, nextTick, ref, useTemplateRef, watch } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { useMediaQuery, useResizeObserver } from '@vueuse/core'
import {
  CircleDashed,
  CornerDownRight,
  Globe,
  HardDrive,
  HeartPulse,
  Network,
  Server,
  Waypoints,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { UpstreamHealthReportResponse, UpstreamView } from '@/api/generated'
import { listSitesOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Badge } from '@/components/ui/badge'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Skeleton } from '@/components/ui/skeleton'
import { latency, nodeTones } from './forms'
import { topology, type TopologyKey } from './topology'

/** The most sites one page of the site list carries. */
const SITE_LIMIT = 500

const props = defineProps<{
  upstreams: UpstreamView[]
  health?: UpstreamHealthReportResponse
  healthFailed: boolean
}>()

const { t } = useI18n()
const sites = useQuery(listSitesOptions({ query: { limit: SITE_LIMIT } }))
const graph = computed(() => topology(sites.data.value?.items ?? [], props.upstreams, props.health))
const poolNames = computed(
  () => new Map(props.upstreams.map((upstream) => [upstream.id, upstream.name])),
)
const truncated = computed(() => {
  const list = sites.data.value
  return list && list.total > list.items.length ? list : undefined
})

// Connectors are drawn only where the columns stand side by side; elsewhere
// each item names what it reaches, which is all the drawing says.
const wide = useMediaQuery('(min-width: 64rem)')
const canvas = useTemplateRef<HTMLElement>('canvas')
const size = ref({ width: 0, height: 0 })
const paths = ref<{ id: string; from: TopologyKey; to: TopologyKey; d: string }[]>([])
const focused = ref<TopologyKey>()

function draw() {
  const root = canvas.value
  if (!root || !wide.value) {
    paths.value = []
    return
  }
  const origin = root.getBoundingClientRect()
  size.value = { width: origin.width, height: origin.height }
  const place = (key: TopologyKey) =>
    root.querySelector(`[data-key="${CSS.escape(key)}"]`)?.getBoundingClientRect()
  paths.value = graph.value.edges.flatMap(([from, to]) => {
    const start = place(from)
    const end = place(to)
    if (!start || !end) {
      return []
    }
    const x1 = start.right - origin.left
    const y1 = start.top + start.height / 2 - origin.top
    const x2 = end.left - origin.left
    const y2 = end.top + end.height / 2 - origin.top
    const bend = Math.max((x2 - x1) / 2, 8)
    return [
      {
        id: `${from}>${to}`,
        from,
        to,
        d: `M ${x1} ${y1} C ${x1 + bend} ${y1}, ${x2 - bend} ${y2}, ${x2} ${y2}`,
      },
    ]
  })
}
watch([graph, wide], () => void nextTick(draw), { immediate: true })
useResizeObserver(canvas, draw)

function highlighted(path: { from: TopologyKey; to: TopologyKey }) {
  return focused.value !== undefined && (path.from === focused.value || path.to === focused.value)
}

function follow(key: TopologyKey) {
  return {
    mouseenter: () => (focused.value = key),
    mouseleave: () => (focused.value = undefined),
    focusin: () => (focused.value = key),
    focusout: () => (focused.value = undefined),
  }
}

const item = 'bg-card relative flex min-w-0 flex-col gap-1 rounded-lg border p-3 text-sm'
</script>

<template>
  <ApiFailureAlert
    v-if="sites.isError.value && !sites.data.value"
    :error="sites.error.value"
    retryable
    @retry="sites.refetch()"
  />
  <div v-else-if="sites.isPending.value" class="grid gap-4 lg:grid-cols-4" aria-busy="true">
    <Skeleton v-for="index in 4" :key="index" class="h-40 w-full" />
  </div>
  <div v-else class="flex flex-col gap-4">
    <p class="text-muted-foreground text-sm">{{ t('upstreams.topology.description') }}</p>
    <Empty v-if="graph.sites.length === 0" class="gap-3 border p-6">
      <EmptyHeader>
        <EmptyMedia variant="icon"><Network aria-hidden="true" /></EmptyMedia>
        <EmptyTitle>{{ t('upstreams.topology.noSites') }}</EmptyTitle>
        <EmptyDescription>{{ t('upstreams.topology.noSitesDetail') }}</EmptyDescription>
      </EmptyHeader>
    </Empty>
    <p v-if="truncated" class="text-muted-foreground text-xs">
      {{
        t('upstreams.topology.firstSites', {
          shown: truncated.items.length,
          total: truncated.total,
        })
      }}
    </p>
    <p v-if="healthFailed" class="text-muted-foreground flex items-center gap-2 text-xs">
      <CircleDashed class="size-3.5" aria-hidden="true" />
      {{ t('upstreams.topology.healthUnavailable') }}
    </p>

    <div ref="canvas" class="relative">
      <svg
        v-if="paths.length"
        class="pointer-events-none absolute inset-0 overflow-visible"
        :width="size.width"
        :height="size.height"
        aria-hidden="true"
        data-testid="topology-connectors"
      >
        <path
          v-for="path in paths"
          :key="path.id"
          :d="path.d"
          fill="none"
          stroke-width="1.5"
          :class="highlighted(path) ? 'stroke-foreground' : 'stroke-border'"
        />
      </svg>
      <div class="grid gap-6 lg:grid-cols-4 lg:gap-12">
        <section class="flex min-w-0 flex-col gap-2" aria-labelledby="topology-sites">
          <h3 id="topology-sites" class="flex items-center gap-2 text-sm font-medium">
            <Globe class="size-4" aria-hidden="true" />{{ t('upstreams.topology.sites') }}
          </h3>
          <ul class="flex flex-col gap-2">
            <li
              v-for="site in graph.sites"
              :key="site.key"
              :data-key="site.key"
              :class="item"
              v-on="follow(site.key)"
            >
              <RouterLink :to="`/sites/${site.id}`" class="truncate font-medium hover:underline">
                {{ site.name }}
              </RouterLink>
              <span class="text-muted-foreground truncate font-mono text-xs">
                {{ site.hosts[0] }}
                <template v-if="site.hosts.length > 1">
                  {{ t('upstreams.topology.moreHosts', { count: site.hosts.length - 1 }) }}
                </template>
              </span>
              <span v-if="site.pool" class="text-muted-foreground flex items-center gap-1 text-xs">
                <CornerDownRight class="size-3.5 shrink-0" aria-hidden="true" />
                {{ t('upstreams.topology.otherRequests', { name: poolNames.get(site.pool) }) }}
              </span>
            </li>
          </ul>
        </section>

        <section class="flex min-w-0 flex-col gap-2" aria-labelledby="topology-routes">
          <h3 id="topology-routes" class="flex items-center gap-2 text-sm font-medium">
            <Waypoints class="size-4" aria-hidden="true" />{{ t('upstreams.topology.routes') }}
          </h3>
          <p v-if="graph.routes.length === 0" class="text-muted-foreground text-xs">
            {{ t('upstreams.topology.noRoutes') }}
          </p>
          <ul v-else class="flex flex-col gap-2">
            <li
              v-for="route in graph.routes"
              :key="route.key"
              :data-key="route.key"
              :class="item"
              v-on="follow(route.key)"
            >
              <span class="flex min-w-0 items-center gap-2">
                <Badge variant="outline">{{ t(`routes.kinds.${route.kind}`) }}</Badge>
                <span class="truncate font-mono text-xs">{{ route.path }}</span>
              </span>
              <span class="text-muted-foreground truncate text-xs">
                {{ route.name ? `${route.name} · ${route.site}` : route.site }}
              </span>
              <span class="text-muted-foreground flex items-center gap-1 text-xs">
                <CornerDownRight class="size-3.5 shrink-0" aria-hidden="true" />
                {{ t('upstreams.topology.sendsTo', { name: poolNames.get(route.pool) }) }}
              </span>
            </li>
          </ul>
        </section>

        <section class="flex min-w-0 flex-col gap-2" aria-labelledby="topology-pools">
          <h3 id="topology-pools" class="flex items-center gap-2 text-sm font-medium">
            <Server class="size-4" aria-hidden="true" />{{ t('upstreams.topology.pools') }}
          </h3>
          <ul class="flex flex-col gap-2">
            <li
              v-for="pool in graph.pools"
              :key="pool.key"
              :data-key="pool.key"
              :class="item"
              v-on="follow(pool.key)"
            >
              <span class="flex min-w-0 items-center justify-between gap-2">
                <RouterLink
                  :to="`/upstreams/${pool.id}`"
                  class="truncate font-medium hover:underline"
                >
                  {{ pool.name }}
                </RouterLink>
                <Badge v-if="!pool.used" variant="outline">{{ t('upstreams.unused') }}</Badge>
              </span>
              <span class="text-muted-foreground flex items-center gap-1.5 text-xs">
                <HeartPulse class="size-3.5 shrink-0" aria-hidden="true" />
                {{ t('upstreams.healthSummary', { healthy: pool.healthy, total: pool.total }) }}
              </span>
            </li>
          </ul>
        </section>

        <section class="flex min-w-0 flex-col gap-2" aria-labelledby="topology-nodes">
          <h3 id="topology-nodes" class="flex items-center gap-2 text-sm font-medium">
            <HardDrive class="size-4" aria-hidden="true" />{{ t('upstreams.topology.nodes') }}
          </h3>
          <ul class="flex flex-col gap-2">
            <li
              v-for="node in graph.nodes"
              :key="node.key"
              :data-key="node.key"
              :class="item"
              v-on="follow(node.key)"
            >
              <span class="flex min-w-0 items-center justify-between gap-2">
                <span class="truncate font-mono text-xs">{{ node.address }}</span>
                <Badge v-if="node.backup" variant="outline">{{ t('upstreams.node.backup') }}</Badge>
              </span>
              <StatusIndicator
                :tone="nodeTones[node.state]"
                :label="t(`upstreams.${node.state}`)"
              />
              <span class="text-muted-foreground truncate text-xs">
                {{ poolNames.get(node.pool) }}
                <template v-if="node.health">
                  · {{ latency(node.health.latency_us) }} ·
                  {{ t('upstreams.topology.inFlight', { count: node.health.in_flight }) }}
                </template>
              </span>
            </li>
          </ul>
        </section>
      </div>
    </div>
  </div>
</template>
