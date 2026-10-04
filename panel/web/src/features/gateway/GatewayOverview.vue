<script setup lang="ts">
import { computed } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import {
  Braces,
  Fingerprint,
  GitCommitHorizontal,
  Layers,
  Plug,
  RefreshCw,
  Server,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { statusOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import CopyValue from '@/components/CopyValue.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import AttentionCard from './AttentionCard.vue'
import DataPlaneCard from './DataPlaneCard.vue'
import FileChecksCard from './FileChecksCard.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Skeleton } from '@/components/ui/skeleton'

const REFRESH_INTERVAL_MS = 10_000

const { t, d } = useI18n()
const status = useQuery({ ...statusOptions(), refetchInterval: REFRESH_INTERVAL_MS })

const updatedAt = computed(() =>
  status.dataUpdatedAt.value > 0 ? d(new Date(status.dataUpdatedAt.value), 'time') : null,
)
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader :icon="Server" :title="t('gateway.title')" :description="t('gateway.description')">
      <template #actions>
        <span v-if="updatedAt" class="text-muted-foreground text-xs">
          {{ t('state.updatedAt', { time: updatedAt }) }}
        </span>
        <Button
          variant="outline"
          size="sm"
          :disabled="status.isFetching.value"
          @click="status.refetch()"
        >
          <RefreshCw
            data-icon="inline-start"
            :class="{ 'animate-spin': status.isFetching.value }"
            aria-hidden="true"
          />
          {{ t('state.refresh') }}
        </Button>
      </template>
    </PageHeader>

    <ApiFailureAlert
      v-if="status.isError.value && !status.data.value"
      :error="status.error.value"
      retryable
      @retry="status.refetch()"
    />

    <div
      v-else-if="status.isPending.value"
      class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3"
      :aria-label="t('state.loading')"
      aria-busy="true"
    >
      <Skeleton v-for="index in 6" :key="index" class="h-28 rounded-xl" />
    </div>

    <template v-else-if="status.data.value">
      <Card>
        <CardHeader>
          <CardDescription>{{ t('gateway.readiness') }}</CardDescription>
          <CardTitle class="flex flex-wrap items-center gap-3">
            <StatusIndicator
              :tone="status.data.value.ready ? 'positive' : 'negative'"
              :label="status.data.value.ready ? t('gateway.ready') : t('gateway.notReady')"
            />
          </CardTitle>
        </CardHeader>
        <CardContent v-if="status.data.value.message" class="text-muted-foreground text-sm">
          {{ status.data.value.message }}
        </CardContent>
      </Card>

      <Empty v-if="status.data.value.active_revision_id == null" class="border border-dashed">
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <GitCommitHorizontal aria-hidden="true" />
          </EmptyMedia>
          <EmptyTitle>{{ t('gateway.noActive') }}</EmptyTitle>
          <EmptyDescription>{{ t('gateway.noActiveDetail') }}</EmptyDescription>
        </EmptyHeader>
      </Empty>

      <dl class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
        <Card v-if="status.data.value.active_revision_id != null">
          <CardHeader>
            <dt class="text-muted-foreground flex items-center gap-2 text-sm">
              <GitCommitHorizontal class="size-4" aria-hidden="true" />
              {{ t('gateway.activeRevision') }}
            </dt>
            <dd class="font-mono text-2xl font-semibold tabular-nums">
              {{ status.data.value.active_revision_id }}
            </dd>
          </CardHeader>
        </Card>
        <Card v-if="status.data.value.active_hash">
          <CardHeader>
            <dt class="text-muted-foreground flex items-center gap-2 text-sm">
              <Fingerprint class="size-4" aria-hidden="true" />
              {{ t('gateway.activeHash') }}
            </dt>
            <dd class="min-w-0"><CopyValue :value="status.data.value.active_hash" /></dd>
          </CardHeader>
        </Card>
        <Card>
          <CardHeader>
            <dt class="text-muted-foreground flex items-center gap-2 text-sm">
              <Layers class="size-4" aria-hidden="true" />
              {{ t('gateway.preparedCount') }}
            </dt>
            <dd class="font-mono text-2xl font-semibold tabular-nums">
              {{ status.data.value.prepared_count }}
            </dd>
          </CardHeader>
        </Card>
        <Card>
          <CardHeader>
            <dt class="text-muted-foreground flex items-center gap-2 text-sm">
              <Plug class="size-4" aria-hidden="true" />
              {{ t('gateway.adapterVersion') }}
            </dt>
            <dd class="font-mono text-lg font-medium">{{ status.data.value.adapter_version }}</dd>
          </CardHeader>
        </Card>
        <Card>
          <CardHeader>
            <dt class="text-muted-foreground flex items-center gap-2 text-sm">
              <Braces class="size-4" aria-hidden="true" />
              {{ t('gateway.schemaVersion') }}
            </dt>
            <dd class="font-mono text-lg font-medium">{{ status.data.value.schema_version }}</dd>
          </CardHeader>
        </Card>
      </dl>
    </template>

    <AttentionCard />
    <DataPlaneCard v-if="status.data.value" />
    <FileChecksCard v-if="status.data.value" />
  </div>
</template>
