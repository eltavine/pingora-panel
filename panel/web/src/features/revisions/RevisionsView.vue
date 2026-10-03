<script setup lang="ts">
import { computed } from 'vue'
import { useInfiniteQuery } from '@tanstack/vue-query'
import { FileCode2, History, MessageSquareText } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { RevisionList } from '@/api/generated'
import { listRevisionsInfiniteOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Button } from '@/components/ui/button'
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from '@/components/ui/empty'
import { Skeleton } from '@/components/ui/skeleton'
import { Spinner } from '@/components/ui/spinner'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { outcomeTones } from './presentation'

const PAGE_SIZE = 50

const { t, d } = useI18n()
const revisions = useInfiniteQuery({
  ...listRevisionsInfiniteOptions({ query: { limit: PAGE_SIZE } }),
  initialPageParam: {} as { query?: { before: number } },
  getNextPageParam: (page: RevisionList) =>
    page.next_before ? { query: { before: page.next_before } } : undefined,
})

const items = computed(() => revisions.data.value?.pages.flatMap((page) => page.items) ?? [])
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="History"
      :title="t('revisions.title')"
      :description="t('revisions.description')"
    />

    <ApiFailureAlert
      v-if="revisions.isError.value && !revisions.data.value"
      :error="revisions.error.value"
      retryable
      @retry="revisions.refetch()"
    />
    <div v-else-if="revisions.isPending.value" class="flex flex-col gap-2">
      <Skeleton v-for="index in 4" :key="index" class="h-12 w-full" />
    </div>

    <Empty v-else-if="items.length === 0" class="border">
      <EmptyHeader>
        <EmptyMedia variant="icon"><History aria-hidden="true" /></EmptyMedia>
        <EmptyTitle>{{ t('revisions.emptyTitle') }}</EmptyTitle>
        <EmptyDescription>{{ t('revisions.emptyDetail') }}</EmptyDescription>
      </EmptyHeader>
      <EmptyContent>
        <Button size="sm" as-child>
          <RouterLink to="/config">
            <FileCode2 data-icon="inline-start" aria-hidden="true" />
            {{ t('nav.config') }}
          </RouterLink>
        </Button>
      </EmptyContent>
    </Empty>

    <template v-else>
      <div class="rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead class="w-20">{{ t('revisions.columns.id') }}</TableHead>
              <TableHead>{{ t('revisions.columns.outcome') }}</TableHead>
              <TableHead>{{ t('revisions.columns.note') }}</TableHead>
              <TableHead>{{ t('revisions.columns.author') }}</TableHead>
              <TableHead>{{ t('revisions.columns.created') }}</TableHead>
              <TableHead class="text-right">{{ t('revisions.columns.draft') }}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-for="revision in items" :key="revision.id">
              <TableCell>
                <RouterLink
                  :to="`/revisions/${revision.id}`"
                  class="font-mono font-medium tabular-nums hover:underline"
                >
                  #{{ revision.id }}
                </RouterLink>
              </TableCell>
              <TableCell>
                <StatusIndicator
                  :tone="outcomeTones[revision.outcome]"
                  :label="t(`revisions.outcome.${revision.outcome}`)"
                />
              </TableCell>
              <TableCell class="max-w-80">
                <span v-if="revision.note" class="flex items-center gap-1.5">
                  <MessageSquareText class="size-3.5 shrink-0" aria-hidden="true" />
                  <span class="truncate">{{ revision.note }}</span>
                </span>
                <span v-else class="text-muted-foreground">{{ t('state.none') }}</span>
              </TableCell>
              <TableCell>{{ revision.author }}</TableCell>
              <TableCell class="whitespace-nowrap">
                <time :datetime="revision.created_at">{{
                  d(new Date(revision.created_at), 'datetime')
                }}</time>
              </TableCell>
              <TableCell class="text-right font-mono tabular-nums">
                v{{ revision.draft_version }}
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>
      <div v-if="revisions.hasNextPage.value" class="flex justify-center">
        <Button
          variant="outline"
          size="sm"
          :disabled="revisions.isFetchingNextPage.value"
          @click="revisions.fetchNextPage()"
        >
          <Spinner v-if="revisions.isFetchingNextPage.value" data-icon="inline-start" />
          {{ t('common.loadMore') }}
        </Button>
      </div>
    </template>
  </div>
</template>
