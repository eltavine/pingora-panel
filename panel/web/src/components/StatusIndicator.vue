<script setup lang="ts">
import { computed, type Component } from 'vue'
import { CircleCheck, CircleDashed, CircleX, LoaderCircle, TriangleAlert } from '@lucide/vue'

export type StatusTone = 'positive' | 'negative' | 'warning' | 'pending' | 'neutral'

const props = defineProps<{
  tone: StatusTone
  label: string
}>()

// The palette is monochrome, so every tone has its own glyph and the label
// is always visible text: state never depends on color.
const icons: Record<StatusTone, Component> = {
  positive: CircleCheck,
  negative: CircleX,
  warning: TriangleAlert,
  pending: LoaderCircle,
  neutral: CircleDashed,
}

const icon = computed(() => icons[props.tone])
</script>

<template>
  <span
    role="status"
    :data-tone="tone"
    class="inline-flex items-center gap-1.5 text-sm font-medium"
    :class="{ 'text-muted-foreground': tone === 'neutral' }"
  >
    <component
      :is="icon"
      class="size-4 shrink-0"
      :class="{ 'animate-spin': tone === 'pending' }"
      aria-hidden="true"
    />
    <span>{{ label }}</span>
  </span>
</template>
