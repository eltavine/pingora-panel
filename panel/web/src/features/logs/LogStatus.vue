<script setup lang="ts">
import { computed, type Component } from 'vue'
import { CircleCheck, CircleDashed, CircleX, TriangleAlert } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { LogRecordItem } from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'
import { toneOf } from './presentation'

const props = defineProps<{ record: LogRecordItem }>()
const { t } = useI18n()

// A record's status in a table row: the same glyphs as status indicators,
// without a live region, as rows arrive while following.
const icons: Partial<Record<StatusTone, Component>> = {
  positive: CircleCheck,
  negative: CircleX,
  warning: TriangleAlert,
}

const tone = computed(() => toneOf(props.record))
const icon = computed(() => icons[tone.value] ?? CircleDashed)
const label = computed(() =>
  props.record.kind === 'error' ? t('logs.error') : String(props.record.status ?? '—'),
)
</script>

<template>
  <span
    :data-tone="tone"
    class="inline-flex items-center gap-1.5 text-sm font-medium tabular-nums"
    :class="{ 'text-muted-foreground': tone === 'neutral' }"
  >
    <component :is="icon" class="size-4 shrink-0" aria-hidden="true" />
    {{ label }}
  </span>
</template>
