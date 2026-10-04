<script setup lang="ts">
import { computed } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { RadioTower } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { hostListenersOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { holderTone, portHolder, REFRESH_INTERVAL_MS } from './presentation'

const { t } = useI18n()
const listeners = useQuery({ ...hostListenersOptions(), refetchInterval: REFRESH_INTERVAL_MS })
const report = computed(() => listeners.data.value)
</script>

<template>
  <Card class="min-w-0">
    <CardHeader>
      <CardTitle class="flex items-center gap-2">
        <RadioTower class="size-4" aria-hidden="true" />{{ t('host.listeners.title') }}
      </CardTitle>
      <CardDescription>{{ t('host.listeners.description') }}</CardDescription>
    </CardHeader>
    <CardContent class="overflow-x-auto">
      <ApiFailureAlert
        v-if="listeners.isError.value && !report"
        :error="listeners.error.value"
        retryable
        @retry="listeners.refetch()"
      />
      <Skeleton
        v-else-if="listeners.isPending.value"
        class="h-20 rounded-lg"
        aria-busy="true"
        :aria-label="t('state.loading')"
      />
      <Table v-else-if="report?.listeners.length">
        <TableHeader>
          <TableRow>
            <TableHead>{{ t('host.listeners.port') }}</TableHead>
            <TableHead>{{ t('host.listeners.processes') }}</TableHead>
            <TableHead>{{ t('host.listeners.holder') }}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow
            v-for="listener in report.listeners"
            :key="`${listener.port}-${listener.address}`"
          >
            <TableCell>
              <div class="flex flex-col">
                <span class="font-medium tabular-nums">{{ listener.port }}</span>
                <span class="text-muted-foreground font-mono text-xs">{{ listener.address }}</span>
              </div>
            </TableCell>
            <TableCell>
              <ul v-if="listener.processes.length" class="flex flex-col gap-1">
                <li v-for="process in listener.processes" :key="process.pid" class="flex flex-col">
                  <span class="text-sm">
                    {{ process.name }}
                    <span class="text-muted-foreground tabular-nums">
                      {{ t('host.listeners.pid', { pid: process.pid }) }}
                    </span>
                  </span>
                  <span
                    v-if="process.executable"
                    class="text-muted-foreground font-mono text-xs break-all"
                  >
                    {{ process.executable }}
                  </span>
                </li>
              </ul>
              <span v-else class="text-muted-foreground text-sm">—</span>
            </TableCell>
            <TableCell>
              <StatusIndicator
                :tone="holderTone(portHolder(listener))"
                :label="t(`host.listeners.holders.${portHolder(listener)}`)"
              />
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
      <p v-else class="text-muted-foreground text-sm">{{ t('host.listeners.empty') }}</p>
    </CardContent>
  </Card>
</template>
