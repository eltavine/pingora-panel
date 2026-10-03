<script setup lang="ts" generic="T extends string">
import type { Component } from 'vue'
import { cn } from '@/lib/utils'

export interface Choice<V extends string> {
  value: V
  label: string
  icon: Component
}

const model = defineModel<T>({ required: true })

defineProps<{
  label: string
  choices: readonly Choice<T>[]
}>()

function move(choices: readonly Choice<T>[], step: number) {
  const index = choices.findIndex((choice) => choice.value === model.value)
  const next = choices[(index + step + choices.length) % choices.length]
  if (next) {
    model.value = next.value
  }
}
</script>

<template>
  <div
    role="radiogroup"
    :aria-label="label"
    :class="
      cn('grid grid-cols-2 gap-2', choices.length === 3 ? 'sm:grid-cols-3' : 'sm:grid-cols-4')
    "
    @keydown.right.prevent="move(choices, 1)"
    @keydown.down.prevent="move(choices, 1)"
    @keydown.left.prevent="move(choices, -1)"
    @keydown.up.prevent="move(choices, -1)"
  >
    <button
      v-for="choice in choices"
      :key="choice.value"
      type="button"
      role="radio"
      :aria-checked="model === choice.value"
      :tabindex="model === choice.value ? 0 : -1"
      :class="
        cn(
          'hover:bg-accent focus-visible:ring-ring/50 flex flex-col items-center gap-2 rounded-md border p-3 text-sm font-medium outline-none focus-visible:ring-3',
          model === choice.value && 'border-primary bg-accent',
        )
      "
      @click="model = choice.value"
    >
      <component :is="choice.icon" class="size-5" aria-hidden="true" />
      {{ choice.label }}
    </button>
  </div>
</template>
