<script setup lang="ts">
import type { Component } from 'vue'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'

const props = defineProps<{
  icon: Component
  label: string
  value?: number
  active?: boolean
  /** A figure only: renders without the button role. */
  passive?: boolean
}>()

const emit = defineEmits<{ select: [] }>()
</script>

<template>
  <component
    :is="props.passive ? 'div' : 'button'"
    :type="props.passive ? undefined : 'button'"
    :aria-pressed="props.passive ? undefined : active"
    :class="
      cn(
        'bg-card flex items-center gap-3 rounded-lg border p-4 text-left',
        !props.passive &&
          'hover:bg-accent focus-visible:ring-ring/50 outline-none focus-visible:ring-3',
        active && 'border-primary',
      )
    "
    @click="props.passive || emit('select')"
  >
    <span
      class="bg-muted flex size-9 shrink-0 items-center justify-center rounded-md"
      aria-hidden="true"
    >
      <component :is="icon" class="size-4" />
    </span>
    <span class="flex min-w-0 flex-col">
      <span class="text-muted-foreground truncate text-xs">{{ label }}</span>
      <Skeleton v-if="value === undefined" class="mt-1 h-6 w-10" />
      <span v-else class="font-heading text-xl font-semibold tabular-nums">{{ value }}</span>
    </span>
  </component>
</template>
