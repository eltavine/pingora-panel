<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { StatusCounts } from '@/api/generated'

const props = defineProps<{
  statuses: StatusCounts
  format: (value: number) => string
}>()

const { t } = useI18n()

const classes = computed(() => {
  const rows = [
    { name: '2xx', count: props.statuses.success, tone: 'fill-foreground' },
    { name: '3xx', count: props.statuses.redirection, tone: 'fill-foreground/60' },
    { name: '4xx', count: props.statuses.client_error, tone: 'fill-foreground/40' },
    { name: '5xx', count: props.statuses.server_error, tone: 'fill-destructive' },
  ]
  const total = rows.reduce((sum, row) => sum + row.count, 0)
  return rows.map((row) => ({ ...row, share: total > 0 ? (row.count / total) * 100 : 0 }))
})
</script>

<template>
  <dl class="flex flex-col gap-3">
    <div
      v-for="row in classes"
      :key="row.name"
      class="grid grid-cols-[3rem_1fr_auto] items-center gap-3"
    >
      <dt class="font-mono text-sm">{{ row.name }}</dt>
      <dd class="min-w-0">
        <svg
          viewBox="0 0 100 4"
          preserveAspectRatio="none"
          class="h-2 w-full"
          role="img"
          :aria-label="t('traffic.statusShare', { status: row.name, share: Math.round(row.share) })"
        >
          <rect width="100" height="4" rx="2" class="fill-muted" />
          <rect :width="row.share" height="4" rx="2" :class="row.tone" />
        </svg>
      </dd>
      <dd class="text-right text-sm tabular-nums">{{ format(row.count) }}</dd>
    </div>
  </dl>
</template>
