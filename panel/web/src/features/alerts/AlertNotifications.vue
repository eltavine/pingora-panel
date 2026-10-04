<script setup lang="ts">
import { computed } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { FilterX, History } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { listAlertNotificationsOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Button } from '@/components/ui/button'
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
import { notificationTone } from './presentation'

const REFRESH_INTERVAL_MS = 15_000
const LIMIT = 100

/** One rule's notifications, or every rule's. */
const rule = defineModel<string>('rule', { default: '' })
const { t, d } = useI18n()

const notifications = useQuery(
  computed(() => ({
    ...listAlertNotificationsOptions({
      query: { rule: rule.value || undefined, limit: LIMIT },
    }),
    refetchInterval: REFRESH_INTERVAL_MS,
  })),
)
const items = computed(() => notifications.data.value ?? [])
</script>

<template>
  <div class="flex flex-col gap-4">
    <div v-if="rule" class="flex flex-wrap items-center justify-between gap-2">
      <p class="text-sm">{{ t('alerts.ofRule', { rule }) }}</p>
      <Button variant="ghost" size="sm" @click="rule = ''">
        <FilterX data-icon="inline-start" aria-hidden="true" />
        {{ t('alerts.everyRule') }}
      </Button>
    </div>
    <ApiFailureAlert
      v-if="notifications.isError.value && !notifications.data.value"
      :error="notifications.error.value"
      retryable
      @retry="notifications.refetch()"
    />
    <div v-else-if="notifications.isPending.value" class="flex flex-col gap-2">
      <Skeleton v-for="index in 3" :key="index" class="h-11 w-full" />
    </div>
    <Empty v-else-if="items.length === 0" class="border">
      <EmptyHeader>
        <EmptyMedia variant="icon"><History aria-hidden="true" /></EmptyMedia>
        <EmptyTitle>{{ t('alerts.noNotificationsTitle') }}</EmptyTitle>
        <EmptyDescription>{{ t('alerts.noNotificationsDetail') }}</EmptyDescription>
      </EmptyHeader>
    </Empty>
    <div v-else class="rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{{ t('alerts.columns.sent') }}</TableHead>
            <TableHead>{{ t('alerts.columns.rule') }}</TableHead>
            <TableHead class="hidden sm:table-cell">{{ t('alerts.columns.channel') }}</TableHead>
            <TableHead>{{ t('alerts.columns.state') }}</TableHead>
            <TableHead class="hidden lg:table-cell">{{ t('alerts.columns.failure') }}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="notification in items" :key="notification.id">
            <TableCell class="whitespace-nowrap">
              <time :datetime="notification.created_at">{{
                d(new Date(notification.created_at), 'datetime')
              }}</time>
            </TableCell>
            <TableCell>
              <div class="flex flex-col">
                <span class="font-mono text-xs">{{ notification.rule }}</span>
                <span class="text-muted-foreground text-xs">
                  {{ t(`alerts.notificationKinds.${notification.kind}`) }}
                </span>
              </div>
            </TableCell>
            <TableCell class="hidden font-mono text-xs sm:table-cell">
              {{ notification.channel }}
            </TableCell>
            <TableCell class="whitespace-nowrap">
              <StatusIndicator
                :tone="notificationTone(notification)"
                :label="
                  t(`alerts.notificationStates.${notification.state}`, {
                    attempts: notification.attempts,
                  })
                "
              />
            </TableCell>
            <TableCell class="text-muted-foreground hidden max-w-72 truncate lg:table-cell">
              {{ notification.last_failure ?? '' }}
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
  </div>
</template>
