<script setup lang="ts">
import { computed } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import {
  Clock,
  Cpu,
  Gauge,
  HardDrive,
  MemoryStick,
  Network,
  RefreshCw,
  ServerCog,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { hostSummaryOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatTile from '@/components/StatTile.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { formatters } from '@/features/traffic/presentation'
import { levelTone, memoryUsed, REFRESH_INTERVAL_MS, uptimeParts } from './presentation'

const { t, d, locale } = useI18n()
const host = useQuery({ ...hostSummaryOptions(), refetchInterval: REFRESH_INTERVAL_MS })
const format = computed(() => formatters(locale.value))
const figures = computed(() => host.data.value)

const updatedAt = computed(() =>
  host.dataUpdatedAt.value > 0 ? d(new Date(host.dataUpdatedAt.value), 'time') : null,
)

const uptime = computed(() => {
  const seconds = figures.value?.uptime_seconds
  return seconds === null || seconds === undefined
    ? '—'
    : uptimeParts(seconds)
        .map((part) => t(`host.${part.unit}`, { n: part.n }, part.n))
        .join(' ')
})

const facts = computed(() => {
  const value = figures.value
  if (!value) {
    return []
  }
  return [
    { label: t('host.hostname'), value: value.hostname },
    { label: t('host.system'), value: value.operating_system },
    { label: t('host.kernel'), value: value.kernel_release },
    { label: t('host.architecture'), value: value.architecture },
    {
      label: t('host.clock'),
      value: value.host_time
        ? `${d(new Date(value.host_time), 'datetime')} (${value.time_zone})`
        : value.time_zone,
    },
  ].filter((fact) => fact.value)
})
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader :icon="ServerCog" :title="t('host.title')" :description="t('host.description')">
      <template #actions>
        <span v-if="updatedAt" class="text-muted-foreground text-xs">
          {{ t('state.updatedAt', { time: updatedAt }) }}
        </span>
        <Button
          variant="outline"
          size="sm"
          :disabled="host.isFetching.value"
          @click="host.refetch()"
        >
          <RefreshCw
            data-icon="inline-start"
            :class="{ 'animate-spin': host.isFetching.value }"
            aria-hidden="true"
          />
          {{ t('state.refresh') }}
        </Button>
      </template>
    </PageHeader>

    <ApiFailureAlert
      v-if="host.isError.value && !figures"
      :error="host.error.value"
      retryable
      @retry="host.refetch()"
    />
    <div
      v-else-if="host.isPending.value"
      class="grid gap-4 sm:grid-cols-2 xl:grid-cols-4"
      aria-busy="true"
      :aria-label="t('state.loading')"
    >
      <Skeleton v-for="index in 4" :key="index" class="h-20 rounded-lg" />
    </div>
    <Empty v-else-if="figures && !figures.reporting" class="border">
      <EmptyHeader>
        <EmptyMedia variant="icon"><ServerCog aria-hidden="true" /></EmptyMedia>
        <EmptyTitle>{{ t('host.notReportingTitle') }}</EmptyTitle>
        <EmptyDescription>{{ t('host.notReportingDetail') }}</EmptyDescription>
      </EmptyHeader>
    </Empty>

    <template v-else-if="figures">
      <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <StatTile
          passive
          :icon="Cpu"
          :label="t('host.cpu', { count: figures.cpu_count }, figures.cpu_count)"
          :value="
            figures.cpu_usage === null || figures.cpu_usage === undefined
              ? '—'
              : format.percent(figures.cpu_usage)
          "
        />
        <StatTile
          passive
          :icon="MemoryStick"
          :label="t('host.memory', { total: format.bytes(figures.memory_total_bytes) })"
          :value="format.percent(memoryUsed(figures))"
        />
        <StatTile
          passive
          :icon="Gauge"
          :label="t('host.load')"
          :value="`${format.rate(figures.load1)} · ${format.rate(figures.load5)} · ${format.rate(figures.load15)}`"
        />
        <StatTile passive :icon="Clock" :label="t('host.uptime')" :value="uptime" />
      </div>

      <Card>
        <CardHeader>
          <CardTitle class="flex items-center gap-2">
            <ServerCog class="size-4" aria-hidden="true" />{{ t('host.systemTitle') }}
          </CardTitle>
        </CardHeader>
        <CardContent>
          <dl class="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[auto_1fr]">
            <template v-for="fact in facts" :key="fact.label">
              <dt class="text-muted-foreground">{{ fact.label }}</dt>
              <dd class="min-w-0 break-words">{{ fact.value }}</dd>
            </template>
          </dl>
        </CardContent>
      </Card>

      <div class="grid gap-4 lg:grid-cols-2">
        <Card class="min-w-0">
          <CardHeader>
            <CardTitle class="flex items-center gap-2">
              <HardDrive class="size-4" aria-hidden="true" />{{ t('host.filesystems') }}
            </CardTitle>
          </CardHeader>
          <CardContent class="overflow-x-auto">
            <Table v-if="figures.filesystems.length">
              <TableHeader>
                <TableRow>
                  <TableHead>{{ t('host.mountpoint') }}</TableHead>
                  <TableHead class="text-right">{{ t('host.size') }}</TableHead>
                  <TableHead>{{ t('host.used') }}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                <TableRow v-for="filesystem in figures.filesystems" :key="filesystem.device">
                  <TableCell>
                    <div class="flex flex-col">
                      <span class="font-mono text-xs font-medium">{{ filesystem.mountpoint }}</span>
                      <span class="text-muted-foreground font-mono text-xs">
                        {{ filesystem.device }} · {{ filesystem.fstype }}
                      </span>
                    </div>
                  </TableCell>
                  <TableCell class="text-right tabular-nums">
                    {{ format.bytes(filesystem.size_bytes) }}
                  </TableCell>
                  <TableCell class="min-w-40">
                    <div class="flex flex-col gap-1">
                      <div
                        class="bg-muted h-1.5 w-full overflow-hidden rounded-full"
                        role="meter"
                        :aria-valuenow="Math.round(filesystem.used_ratio * 100)"
                        aria-valuemin="0"
                        aria-valuemax="100"
                        :aria-label="t('host.usedOf', { mountpoint: filesystem.mountpoint })"
                      >
                        <div
                          class="h-full rounded-full"
                          :class="filesystem.level === 'ok' ? 'bg-foreground/60' : 'bg-destructive'"
                          :style="{ width: `${Math.round(filesystem.used_ratio * 100)}%` }"
                        />
                      </div>
                      <StatusIndicator
                        :tone="levelTone(filesystem.level)"
                        :label="
                          t(`host.levels.${filesystem.level}`, {
                            used: format.percent(filesystem.used_ratio),
                          })
                        "
                      />
                    </div>
                  </TableCell>
                </TableRow>
              </TableBody>
            </Table>
            <p v-else class="text-muted-foreground text-sm">{{ t('host.noFilesystems') }}</p>
          </CardContent>
        </Card>
        <Card class="min-w-0">
          <CardHeader>
            <CardTitle class="flex items-center gap-2">
              <Network class="size-4" aria-hidden="true" />{{ t('host.network') }}
            </CardTitle>
          </CardHeader>
          <CardContent class="overflow-x-auto">
            <Table v-if="figures.network_devices.length">
              <TableHeader>
                <TableRow>
                  <TableHead>{{ t('host.device') }}</TableHead>
                  <TableHead class="text-right">{{ t('host.receive') }}</TableHead>
                  <TableHead class="text-right">{{ t('host.transmit') }}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                <TableRow v-for="device in figures.network_devices" :key="device.device">
                  <TableCell class="font-mono text-xs font-medium">{{ device.device }}</TableCell>
                  <TableCell class="text-right tabular-nums">
                    {{ format.bytes(device.receive_bytes_per_second) }}/s
                  </TableCell>
                  <TableCell class="text-right tabular-nums">
                    {{ format.bytes(device.transmit_bytes_per_second) }}/s
                  </TableCell>
                </TableRow>
              </TableBody>
            </Table>
            <p v-else class="text-muted-foreground text-sm">{{ t('host.noDevices') }}</p>
          </CardContent>
        </Card>
      </div>
    </template>
  </div>
</template>
