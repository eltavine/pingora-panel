<script setup lang="ts">
import { computed, type Component } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { FileCog, FolderTree, ScrollText, ShieldCheck } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { DirectoryKindName } from '@/api/generated'
import { hostDirectoriesOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
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
import { DIRECTORY_REFRESH_MS, directoryNotes, type DirectoryNote } from './presentation'

const { t, locale } = useI18n()
const directories = useQuery({
  ...hostDirectoriesOptions(),
  refetchInterval: DIRECTORY_REFRESH_MS,
})
const format = computed(() => formatters(locale.value))
const report = computed(() => directories.data.value)

const icons: Record<DirectoryKindName, Component> = {
  configuration: FileCog,
  logs: ScrollText,
  certificates: ShieldCheck,
}

function noteLabel(note: DirectoryNote): string {
  switch (note.kind) {
    case 'missing':
      return t('host.directories.missing')
    case 'partial':
      return t('host.directories.partial')
    case 'unreadable':
      return t('host.directories.unreadable', { n: note.n }, note.n)
  }
}
</script>

<template>
  <Card class="min-w-0">
    <CardHeader>
      <CardTitle class="flex items-center gap-2">
        <FolderTree class="size-4" aria-hidden="true" />{{ t('host.directories.title') }}
      </CardTitle>
    </CardHeader>
    <CardContent class="overflow-x-auto">
      <ApiFailureAlert
        v-if="directories.isError.value && !report"
        :error="directories.error.value"
        retryable
        @retry="directories.refetch()"
      />
      <Skeleton
        v-else-if="directories.isPending.value"
        class="h-24 rounded-lg"
        aria-busy="true"
        :aria-label="t('state.loading')"
      />
      <Table v-else-if="report?.directories.length">
        <TableHeader>
          <TableRow>
            <TableHead>{{ t('host.directories.kind') }}</TableHead>
            <TableHead class="text-right">{{ t('host.directories.size') }}</TableHead>
            <TableHead class="text-right">{{ t('host.directories.files') }}</TableHead>
            <TableHead>{{ t('host.directories.note') }}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="directory in report.directories" :key="directory.kind">
            <TableCell>
              <div class="flex items-start gap-2">
                <component
                  :is="icons[directory.kind]"
                  class="text-muted-foreground mt-0.5 size-4 shrink-0"
                  aria-hidden="true"
                />
                <div class="flex min-w-0 flex-col">
                  <span class="text-sm font-medium">
                    {{ t(`host.directories.kinds.${directory.kind}`) }}
                  </span>
                  <span class="text-muted-foreground font-mono text-xs break-all">
                    {{ directory.path }}
                  </span>
                </div>
              </div>
            </TableCell>
            <TableCell class="text-right tabular-nums">
              {{ directory.present ? format.bytes(directory.bytes) : '—' }}
            </TableCell>
            <TableCell class="text-right tabular-nums">
              {{ directory.present ? format.count(directory.files) : '—' }}
            </TableCell>
            <TableCell>
              <div class="flex flex-wrap gap-x-3 gap-y-1">
                <StatusIndicator
                  v-for="note in directoryNotes(directory)"
                  :key="note.kind"
                  :tone="note.kind === 'missing' ? 'neutral' : 'warning'"
                  :label="noteLabel(note)"
                  :title="note.kind === 'partial' ? t('host.directories.partialDetail') : undefined"
                />
              </div>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
      <p v-else class="text-muted-foreground text-sm">{{ t('host.directories.empty') }}</p>
    </CardContent>
  </Card>
</template>
