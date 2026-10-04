<script setup lang="ts">
import { computed } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { History } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { LogDeletionItem } from '@/api/generated'
import { listLogDeletionsOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'

const open = defineModel<boolean>('open', { required: true })

const { t, d } = useI18n()
const deletions = useQuery(computed(() => ({ ...listLogDeletionsOptions(), enabled: open.value })))
const items = computed(() => deletions.data.value?.deletions ?? [])

/** Deletions without a start reach back to the first record. */
function since(deletion: LogDeletionItem): string {
  return Date.parse(deletion.since) === 0
    ? t('logs.fromTheFirst')
    : d(new Date(deletion.since), 'datetime')
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2">
          <History class="size-5" aria-hidden="true" />
          {{ t('logs.deletions') }}
        </SheetTitle>
        <SheetDescription>{{ t('logs.deletionsDescription') }}</SheetDescription>
      </SheetHeader>
      <div class="px-4 pb-4">
        <ApiFailureAlert
          v-if="deletions.isError.value"
          :error="deletions.error.value"
          retryable
          @retry="deletions.refetch()"
        />
        <div v-else-if="deletions.isPending.value" class="flex flex-col gap-2">
          <Skeleton v-for="index in 3" :key="index" class="h-16 w-full" />
        </div>
        <Empty v-else-if="items.length === 0" class="border">
          <EmptyHeader>
            <EmptyMedia variant="icon"><History aria-hidden="true" /></EmptyMedia>
            <EmptyTitle>{{ t('logs.noDeletionsTitle') }}</EmptyTitle>
            <EmptyDescription>{{ t('logs.noDeletionsDetail') }}</EmptyDescription>
          </EmptyHeader>
        </Empty>
        <ul v-else class="flex flex-col divide-y rounded-lg border">
          <li
            v-for="deletion in items"
            :key="`${deletion.requested_at}-${deletion.site ?? ''}`"
            class="flex flex-col gap-1 p-3 text-sm"
          >
            <div class="flex flex-wrap items-center justify-between gap-2">
              <span class="font-medium">{{ deletion.site ?? t('logs.allSites') }}</span>
              <StatusIndicator
                :tone="deletion.state === 'applied' ? 'positive' : 'pending'"
                :label="t(`logs.deletionStates.${deletion.state}`)"
              />
            </div>
            <span class="text-muted-foreground text-xs">
              {{
                t('logs.deletionRange', {
                  since: since(deletion),
                  until: d(new Date(deletion.until), 'datetime'),
                })
              }}
            </span>
            <span class="text-muted-foreground text-xs">
              {{ t('logs.requestedAt', { time: d(new Date(deletion.requested_at), 'datetime') }) }}
            </span>
          </li>
        </ul>
      </div>
    </SheetContent>
  </Sheet>
</template>
