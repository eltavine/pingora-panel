<script setup lang="ts">
import { computed, type Component } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import {
  BellRing,
  ChevronRight,
  CircleCheck,
  ShieldAlert,
  Siren,
  TriangleAlert,
  Unplug,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { RouterLink } from 'vue-router'
import {
  listAlertRulesOptions,
  listAutomaticCertificatesOptions,
  listCertificatesOptions,
  searchLogsOptions,
  trafficSummaryOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { formatMeasure } from '@/lib/alerts'
import { summaryOf } from '@/lib/logs'
import { nodeOf } from '@/lib/format'
import { useSession } from '@/lib/session'

const REFRESH_INTERVAL_MS = 30_000
/** Items shown in each section; the linked page has the rest. */
const SHOWN = 5
const UPSTREAM_WINDOW_SECONDS = 900

const { t, d, locale } = useI18n()
const { can } = useSession()

const alerts = useQuery(
  computed(() => ({
    ...listAlertRulesOptions(),
    enabled: can('alerts.read'),
    refetchInterval: REFRESH_INTERVAL_MS,
  })),
)
const failed = useQuery(
  computed(() => ({
    ...searchLogsOptions({ query: { status: '5xx', limit: SHOWN } }),
    enabled: can('logs.read'),
    refetchInterval: REFRESH_INTERVAL_MS,
  })),
)
const traffic = useQuery(
  computed(() => ({
    ...trafficSummaryOptions({ query: { window: UPSTREAM_WINDOW_SECONDS } }),
    enabled: can('gateway.read'),
    refetchInterval: REFRESH_INTERVAL_MS,
  })),
)
const certificates = useQuery(
  computed(() => ({ ...listCertificatesOptions(), enabled: can('certificate.read') })),
)
const automatic = useQuery(
  computed(() => ({ ...listAutomaticCertificatesOptions(), enabled: can('certificate.read') })),
)

interface Item {
  key: string
  title: string
  detail: string
  /** RFC 3339. */
  at?: string
}

interface Section {
  id: string
  icon: Component
  title: string
  items: Item[]
  /** Whether the section's data has arrived. */
  ready: boolean
  calm: string
  more: string
  to: string
}

const firingItems = computed<Item[]>(() =>
  (alerts.data.value ?? [])
    .filter((rule) => rule.spec.enabled && rule.state === 'firing')
    .slice(0, SHOWN)
    .map((rule) => ({
      key: rule.id,
      title: rule.spec.name,
      detail: `${t(`alerts.measures.${rule.spec.measure}`)}: ${formatMeasure(
        rule.spec.measure,
        rule.value,
        locale.value,
      )}`,
      at: rule.since ?? undefined,
    })),
)

const failedItems = computed<Item[]>(() =>
  (failed.data.value?.records ?? []).map((record, index) => ({
    key: `${record.time}-${index}`,
    title: [record.status, record.site].filter(Boolean).join(' · ') || t('logs.error'),
    detail: summaryOf(record),
    at: record.time,
  })),
)

const upstreamItems = computed<Item[]>(() =>
  (traffic.data.value?.upstream_failures ?? []).slice(0, SHOWN).map((failure) => ({
    key: `${failure.upstream}/${failure.address}:${failure.port}/${failure.error_type}`,
    title: `${failure.upstream} · ${nodeOf(failure)}`,
    detail: t(
      'gateway.attention.upstreamFailed',
      { error: failure.error_type, count: Math.round(failure.failures) },
      Math.round(failure.failures),
    ),
  })),
)

const certificateItems = computed<Item[]>(() => {
  const inventory = (certificates.data.value ?? [])
    .filter((certificate) => certificate.status === 'expired' || certificate.status === 'expiring')
    .map((certificate) => ({
      key: `certificate/${certificate.id}`,
      title: certificate.names[0] ?? certificate.id,
      detail:
        certificate.status === 'expired'
          ? t('gateway.attention.expired', { time: d(new Date(certificate.not_after), 'datetime') })
          : t('gateway.attention.expiring', {
              time: d(new Date(certificate.not_after), 'datetime'),
            }),
      at: certificate.not_after,
    }))
  const renewals = (automatic.data.value ?? [])
    .filter((issued) => issued.state === 'failing' && issued.last_error)
    .map((issued) => ({
      key: `automatic/${issued.id}`,
      title: issued.names[0] ?? issued.id,
      detail: issued.last_error?.message ?? '',
      at: issued.last_error?.at,
    }))
  return [...renewals, ...inventory].slice(0, SHOWN)
})

const sections = computed(() => {
  const shown: Section[] = []
  if (can('alerts.read')) {
    shown.push({
      id: 'alerts',
      icon: Siren,
      title: t('gateway.attention.firingAlerts'),
      items: firingItems.value,
      ready: alerts.isSuccess.value,
      calm: t('gateway.attention.noFiringAlerts'),
      more: t('gateway.attention.showAlerts'),
      to: '/alerts',
    })
  }
  if (can('logs.read')) {
    shown.push({
      id: 'failed',
      icon: TriangleAlert,
      title: t('gateway.attention.failedRequests'),
      items: failedItems.value,
      ready: failed.isSuccess.value,
      calm: t('gateway.attention.noFailedRequests'),
      more: t('gateway.attention.showFailedRequests'),
      to: '/logs?status=5xx',
    })
  }
  if (can('gateway.read')) {
    shown.push({
      id: 'upstreams',
      icon: Unplug,
      title: t('gateway.attention.upstreamFailures'),
      items: upstreamItems.value,
      ready: traffic.isSuccess.value,
      calm: t('gateway.attention.noUpstreamFailures'),
      more: t('gateway.attention.showTraffic'),
      to: `/traffic?window=${UPSTREAM_WINDOW_SECONDS}`,
    })
  }
  if (can('certificate.read')) {
    shown.push({
      id: 'certificates',
      icon: ShieldAlert,
      title: t('gateway.attention.certificateProblems'),
      items: certificateItems.value,
      ready: certificates.isSuccess.value && automatic.isSuccess.value,
      calm: t('gateway.attention.noCertificateProblems'),
      more: t('gateway.attention.showCertificates'),
      to: '/certificates',
    })
  }
  return shown
})
</script>

<template>
  <Card v-if="sections.length > 0">
    <CardHeader>
      <CardTitle class="flex items-center gap-2">
        <BellRing class="size-4" aria-hidden="true" />{{ t('gateway.attention.title') }}
      </CardTitle>
      <CardDescription>{{ t('gateway.attention.description') }}</CardDescription>
    </CardHeader>
    <CardContent class="grid gap-6 md:grid-cols-2 xl:grid-cols-4">
      <section
        v-for="section in sections"
        :key="section.id"
        class="flex min-w-0 flex-col gap-2"
        :aria-labelledby="`attention-${section.id}`"
      >
        <h3 :id="`attention-${section.id}`" class="flex items-center gap-2 text-sm font-medium">
          <component :is="section.icon" class="size-4 shrink-0" aria-hidden="true" />
          {{ section.title }}
        </h3>
        <p v-if="!section.ready" class="text-muted-foreground text-sm">
          {{ t('state.loading') }}
        </p>
        <p
          v-else-if="section.items.length === 0"
          class="text-muted-foreground flex items-center gap-2 text-sm"
        >
          <CircleCheck class="size-4 shrink-0" aria-hidden="true" />{{ section.calm }}
        </p>
        <ul v-else class="flex flex-col divide-y rounded-lg border">
          <li v-for="item in section.items" :key="item.key" class="flex flex-col gap-0.5 p-2.5">
            <div class="flex items-baseline justify-between gap-2">
              <span class="min-w-0 truncate text-sm font-medium">{{ item.title }}</span>
              <time
                v-if="item.at"
                :datetime="item.at"
                class="text-muted-foreground shrink-0 text-xs tabular-nums"
                >{{ d(new Date(item.at), 'datetime') }}</time
              >
            </div>
            <span class="text-muted-foreground truncate font-mono text-xs" :title="item.detail">
              {{ item.detail }}
            </span>
          </li>
        </ul>
        <Button variant="ghost" size="sm" class="self-start" as-child>
          <RouterLink :to="section.to">
            {{ section.more }}
            <ChevronRight data-icon="inline-end" aria-hidden="true" />
          </RouterLink>
        </Button>
      </section>
    </CardContent>
  </Card>
</template>
