<script setup lang="ts">
import { computed } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { Network } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { listNetworksOptions } from '@/api/generated/@tanstack/vue-query.gen'
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
const networks = useQuery(
  computed(() => ({
    ...listNetworksOptions({ path: { engine: props.engine } }),
    refetchInterval: REFRESH_INTERVAL_MS,
  })),
)
const rows = computed(() => networks.data.value?.networks ?? [])
</script>

<template>
  <ApiFailureAlert
    v-if="networks.isError.value && !networks.data.value"
    :error="networks.error.value"
    retryable
    @retry="networks.refetch()"
  />
  <Skeleton
    v-else-if="networks.isPending.value"
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
          <TableHead>{{ t('containers.networks.subnets') }}</TableHead>
          <TableHead>{{ t('containers.images.containers') }}</TableHead>
          <TableHead class="hidden md:table-cell">{{ t('containers.networks.scope') }}</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="network in rows" :key="network.id">
          <TableCell>
            <div class="flex flex-col items-start gap-1">
              <span class="font-medium break-all">{{ network.name }}</span>
              <span class="flex flex-wrap gap-1">
                <Badge v-if="network.compose_project" variant="secondary">
                  {{ t('containers.list.project', { project: network.compose_project }) }}
                </Badge>
                <Badge v-if="network.internal" variant="outline">
                  {{ t('containers.networks.internal') }}
                </Badge>
                <Badge v-if="network.ipv6" variant="outline">IPv6</Badge>
              </span>
            </div>
          </TableCell>
          <TableCell class="font-mono text-xs">{{ network.driver }}</TableCell>
          <TableCell>
            <ul v-if="network.subnets.length" class="flex flex-col gap-0.5">
              <li
                v-for="subnet in network.subnets"
                :key="subnet.subnet"
                class="font-mono text-xs whitespace-nowrap"
              >
                {{
                  subnet.gateway
                    ? t('containers.networks.via', {
                        subnet: subnet.subnet,
                        gateway: subnet.gateway,
                      })
                    : subnet.subnet
                }}
              </li>
            </ul>
            <span v-else class="text-muted-foreground text-sm">—</span>
          </TableCell>
          <TableCell class="text-sm whitespace-nowrap">
            <span :class="{ 'text-muted-foreground': network.containers === 0 }">
              {{ t('containers.images.used', { n: network.containers }, network.containers) }}
            </span>
          </TableCell>
          <TableCell class="text-muted-foreground hidden text-sm md:table-cell">
            {{ network.scope }}
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  </div>
  <Empty v-else class="border border-dashed">
    <EmptyHeader>
      <EmptyMedia variant="icon"><Network aria-hidden="true" /></EmptyMedia>
      <EmptyTitle>{{ t('containers.networks.empty') }}</EmptyTitle>
    </EmptyHeader>
  </Empty>
</template>
