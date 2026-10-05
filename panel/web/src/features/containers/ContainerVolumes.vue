<script setup lang="ts">
import { computed } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { HardDrive } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { listVolumesOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import { Badge } from '@/components/ui/badge'
import { Empty, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { REFRESH_INTERVAL_MS } from './presentation'

const props = defineProps<{ engine: string }>()

const { t } = useI18n()
const volumes = useQuery(
  computed(() => ({
    ...listVolumesOptions({ path: { engine: props.engine } }),
    refetchInterval: REFRESH_INTERVAL_MS,
  })),
)
const rows = computed(() => volumes.data.value?.volumes ?? [])
</script>

<template>
  <ApiFailureAlert
    v-if="volumes.isError.value && !volumes.data.value"
    :error="volumes.error.value"
    retryable
    @retry="volumes.refetch()"
  />
  <Skeleton
    v-else-if="volumes.isPending.value"
    class="h-24 rounded-lg"
    aria-busy="true"
    :aria-label="t('state.loading')"
  />
  <div v-else-if="rows.length" class="overflow-x-auto">
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>{{ t('containers.networks.name') }}</TableHead>
          <TableHead>{{ t('containers.networks.driver') }}</TableHead>
          <TableHead>{{ t('containers.images.containers') }}</TableHead>
          <TableHead class="hidden md:table-cell">{{
            t('containers.volumes.mountpoint')
          }}</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="volume in rows" :key="volume.name">
          <TableCell>
            <div class="flex flex-col items-start gap-1">
              <span class="font-mono text-xs break-all">{{ volume.name }}</span>
              <Badge v-if="volume.compose_project" variant="secondary">
                {{ t('containers.list.project', { project: volume.compose_project }) }}
              </Badge>
            </div>
          </TableCell>
          <TableCell class="font-mono text-xs">{{ volume.driver }}</TableCell>
          <TableCell class="text-sm whitespace-nowrap">
            <span :class="{ 'text-muted-foreground': volume.containers === 0 }">
              {{ t('containers.images.used', { n: volume.containers }, volume.containers) }}
            </span>
          </TableCell>
          <TableCell class="text-muted-foreground hidden font-mono text-xs break-all md:table-cell">
            {{ volume.mountpoint }}
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  </div>
  <Empty v-else class="border border-dashed">
    <EmptyHeader>
      <EmptyMedia variant="icon"><HardDrive aria-hidden="true" /></EmptyMedia>
      <EmptyTitle>{{ t('containers.volumes.empty') }}</EmptyTitle>
    </EmptyHeader>
  </Empty>
</template>
