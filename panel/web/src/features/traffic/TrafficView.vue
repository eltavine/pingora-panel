<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useQuery } from '@tanstack/vue-query'
import {
  Activity,
  ArrowDownToLine,
  ArrowUpFromLine,
  ChartBar,
  Cable,
  Gauge,
  GitCommitHorizontal,
  Hash,
  LockKeyhole,
  RefreshCw,
  Route as RouteIcon,
  Server,
  Timer,
  TriangleAlert,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import {
  listSitesOptions,
  trafficSeriesOptions,
  trafficSummaryOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatTile from '@/components/StatTile.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { useSession } from '@/lib/session'
import StatusBreakdown from './StatusBreakdown.vue'
import TrafficChart from './TrafficChart.vue'
import { formatters, REFRESH_INTERVAL_MS, WINDOWS } from './presentation'

const ALL = '*'

const { t, d, locale } = useI18n()
const { can } = useSession()
const route = useRoute()
const router = useRouter()

const windowSeconds = computed(() => {
  const requested = Number(route.query.window)
  return (WINDOWS as readonly number[]).includes(requested) ? requested : WINDOWS[0]
})
const site = computed(() =>
  typeof route.query.site === 'string' && route.query.site ? route.query.site : undefined,
)

function choose(name: 'window' | 'site', value: string) {
  const query = { ...route.query, [name]: value === ALL ? undefined : value }
  void router.replace({ query })
}

const scope = computed(() => ({ window: windowSeconds.value, site: site.value }))
const summary = useQuery(
  computed(() => ({
    ...trafficSummaryOptions({ query: scope.value }),
    refetchInterval: REFRESH_INTERVAL_MS,
  })),
)
const series = useQuery(
  computed(() => ({
    ...trafficSeriesOptions({ query: scope.value }),
    refetchInterval: REFRESH_INTERVAL_MS,
  })),
)
const sites = useQuery(
  computed(() => ({
    ...listSitesOptions({ query: { limit: 500 } }),
    enabled: can('config.read'),
  })),
)

const format = computed(() => formatters(locale.value))
const figures = computed(() => summary.data.value)
const points = computed(() => series.data.value?.points ?? [])
const quiet = computed(
  () => figures.value !== undefined && figures.value.requests === 0 && points.value.length === 0,
)
const times = computed(() =>
  points.value.map((point) =>
    d(new Date(point.at), windowSeconds.value >= 86_400 ? 'datetime' : 'time'),
  ),
)
const windowLabel = (seconds: number) =>
  seconds % 86_400 === 0
    ? t('traffic.days', seconds / 86_400)
    : seconds % 3_600 === 0
      ? t('traffic.hours', seconds / 3_600)
      : t('traffic.minutes', seconds / 60)
const updatedAt = computed(() =>
  summary.dataUpdatedAt.value > 0 ? d(new Date(summary.dataUpdatedAt.value), 'time') : null,
)

function refresh() {
  void summary.refetch()
  void series.refetch()
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="Activity"
      :title="t('traffic.title')"
      :description="t('traffic.description')"
    >
      <template #actions>
        <span v-if="updatedAt" class="text-muted-foreground text-xs">
          {{ t('state.updatedAt', { time: updatedAt }) }}
        </span>
        <Select
          v-if="can('config.read')"
          :model-value="site ?? ALL"
          @update:model-value="(value) => choose('site', String(value))"
        >
          <SelectTrigger class="w-40" :aria-label="t('traffic.site')">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem :value="ALL">{{ t('traffic.allSites') }}</SelectItem>
            <SelectItem
              v-for="known in sites.data.value?.items ?? []"
              :key="known.id"
              :value="known.id"
            >
              {{ known.name }}
            </SelectItem>
          </SelectContent>
        </Select>
        <Select
          :model-value="String(windowSeconds)"
          @update:model-value="(value) => choose('window', String(value))"
        >
          <SelectTrigger class="w-32" :aria-label="t('traffic.window')">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem v-for="seconds in WINDOWS" :key="seconds" :value="String(seconds)">
              {{ windowLabel(seconds) }}
            </SelectItem>
          </SelectContent>
        </Select>
        <Button variant="outline" size="sm" :disabled="summary.isFetching.value" @click="refresh">
          <RefreshCw
            data-icon="inline-start"
            :class="{ 'animate-spin': summary.isFetching.value }"
            aria-hidden="true"
          />
          {{ t('state.refresh') }}
        </Button>
      </template>
    </PageHeader>

    <ApiFailureAlert
      v-if="summary.isError.value && !figures"
      :error="summary.error.value"
      retryable
      @retry="refresh"
    />

    <div
      v-else-if="summary.isPending.value"
      class="grid gap-4 sm:grid-cols-2 xl:grid-cols-4"
      :aria-label="t('state.loading')"
      aria-busy="true"
    >
      <Skeleton v-for="index in 8" :key="index" class="h-20 rounded-lg" />
    </div>

    <template v-else-if="figures">
      <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <StatTile
          passive
          :icon="Hash"
          :label="t('traffic.requests')"
          :value="format.count(figures.requests)"
        />
        <StatTile
          passive
          :icon="Gauge"
          :label="t('traffic.requestsPerSecond')"
          :value="format.rate(figures.requests_per_second)"
        />
        <StatTile
          passive
          :icon="TriangleAlert"
          :label="t('traffic.serverErrors')"
          :value="format.count(figures.statuses.server_error)"
        />
        <StatTile
          passive
          :icon="Timer"
          :label="t('traffic.latencyP95')"
          :value="format.seconds(figures.latency.p95)"
        />
        <StatTile
          passive
          :icon="ArrowDownToLine"
          :label="t('traffic.received')"
          :value="format.bytes(figures.bytes_received)"
        />
        <StatTile
          passive
          :icon="ArrowUpFromLine"
          :label="t('traffic.sent')"
          :value="format.bytes(figures.bytes_sent)"
        />
        <StatTile
          passive
          :icon="Cable"
          :label="t('traffic.connections')"
          :value="format.count(figures.open_connections)"
        />
        <StatTile
          passive
          :icon="LockKeyhole"
          :label="t('traffic.handshakes')"
          :value="format.count(figures.tls_handshakes)"
        />
      </div>

      <Empty v-if="quiet" class="border">
        <EmptyHeader>
          <EmptyMedia variant="icon"><Activity /></EmptyMedia>
          <EmptyTitle>{{ t('traffic.quietTitle') }}</EmptyTitle>
          <EmptyDescription>{{ t('traffic.quietDescription') }}</EmptyDescription>
        </EmptyHeader>
      </Empty>

      <div v-else class="grid gap-4 lg:grid-cols-2">
        <Card class="min-w-0">
          <CardHeader>
            <CardTitle>{{ t('traffic.rateChart') }}</CardTitle>
            <CardDescription>{{ t('traffic.rateChartDescription') }}</CardDescription>
          </CardHeader>
          <CardContent>
            <TrafficChart
              :label="t('traffic.rateChart')"
              :times="times"
              :format="format.rate"
              :series="[
                {
                  name: t('traffic.requestsPerSecond'),
                  values: points.map((point) => point.requests_per_second),
                  tone: 'foreground',
                },
                {
                  name: t('traffic.serverErrorsPerSecond'),
                  values: points.map((point) => point.server_errors_per_second),
                  tone: 'destructive',
                },
              ]"
            />
          </CardContent>
        </Card>
        <Card class="min-w-0">
          <CardHeader>
            <CardTitle>{{ t('traffic.latencyChart') }}</CardTitle>
            <CardDescription>
              {{
                t('traffic.latencyQuantiles', {
                  p50: format.seconds(figures.latency.p50),
                  p90: format.seconds(figures.latency.p90),
                  p99: format.seconds(figures.latency.p99),
                })
              }}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <TrafficChart
              :label="t('traffic.latencyChart')"
              :times="times"
              :format="format.seconds"
              :series="[
                {
                  name: t('traffic.latencyP95'),
                  values: points.map((point) => point.p95 ?? null),
                  tone: 'foreground',
                },
              ]"
            />
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardTitle class="flex items-center gap-2">
            <ChartBar class="size-4" aria-hidden="true" />{{ t('traffic.statuses') }}
          </CardTitle>
        </CardHeader>
        <CardContent>
          <StatusBreakdown :statuses="figures.statuses" :format="format.count" />
        </CardContent>
      </Card>

      <div class="grid gap-4 lg:grid-cols-2">
        <Card class="min-w-0">
          <CardHeader>
            <CardTitle class="flex items-center gap-2">
              <Server class="size-4" aria-hidden="true" />{{ t('traffic.upstreams') }}
            </CardTitle>
          </CardHeader>
          <CardContent class="overflow-x-auto">
            <Table v-if="figures.upstreams.length">
              <TableHeader>
                <TableRow>
                  <TableHead>{{ t('traffic.upstream') }}</TableHead>
                  <TableHead class="text-right">{{ t('traffic.requests') }}</TableHead>
                  <TableHead class="text-right">{{ t('traffic.errorRatio') }}</TableHead>
                  <TableHead class="text-right">{{ t('traffic.latencyP95') }}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                <TableRow v-for="upstream in figures.upstreams" :key="upstream.upstream">
                  <TableCell class="font-medium">{{ upstream.upstream }}</TableCell>
                  <TableCell class="text-right tabular-nums">
                    {{ format.count(upstream.requests) }}
                  </TableCell>
                  <TableCell
                    :class="[
                      'text-right tabular-nums',
                      upstream.error_ratio > 0 && 'text-destructive',
                    ]"
                  >
                    {{ format.percent(upstream.error_ratio) }}
                  </TableCell>
                  <TableCell class="text-right tabular-nums">
                    {{ format.seconds(upstream.latency.p95) }}
                  </TableCell>
                </TableRow>
              </TableBody>
            </Table>
            <p v-else class="text-muted-foreground text-sm">{{ t('traffic.noUpstreams') }}</p>
          </CardContent>
        </Card>
        <Card class="min-w-0">
          <CardHeader>
            <CardTitle class="flex items-center gap-2">
              <RouteIcon class="size-4" aria-hidden="true" />{{ t('traffic.routes') }}
            </CardTitle>
          </CardHeader>
          <CardContent class="overflow-x-auto">
            <Table v-if="figures.routes.length">
              <TableHeader>
                <TableRow>
                  <TableHead>{{ t('traffic.site') }}</TableHead>
                  <TableHead>{{ t('traffic.route') }}</TableHead>
                  <TableHead class="text-right">{{ t('traffic.requests') }}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                <TableRow
                  v-for="busiest in figures.routes"
                  :key="`${busiest.site}/${busiest.route}`"
                >
                  <TableCell>{{ busiest.site }}</TableCell>
                  <TableCell class="font-medium">{{ busiest.route }}</TableCell>
                  <TableCell class="text-right tabular-nums">
                    {{ format.count(busiest.requests) }}
                  </TableCell>
                </TableRow>
              </TableBody>
            </Table>
            <p v-else class="text-muted-foreground text-sm">{{ t('traffic.noRoutes') }}</p>
          </CardContent>
        </Card>
      </div>

      <p
        v-if="figures.revision !== null && figures.revision !== undefined"
        class="text-muted-foreground flex items-center gap-2 text-xs"
      >
        <GitCommitHorizontal class="size-4" aria-hidden="true" />
        {{
          t('traffic.revision', {
            revision: figures.revision,
            time: figures.activated_at ? d(new Date(figures.activated_at), 'datetime') : '—',
          })
        }}
      </p>
    </template>
  </div>
</template>
