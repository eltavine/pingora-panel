<script setup lang="ts">
import type { Component } from 'vue'

defineProps<{
  icon: Component
  title: string
  description?: string
}>()
</script>

<template>
  <header class="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
    <div class="flex min-w-0 items-start gap-3">
      <div
        class="bg-primary text-primary-foreground flex size-10 shrink-0 items-center justify-center rounded-lg"
        aria-hidden="true"
      >
        <component :is="icon" class="size-5" />
      </div>
      <div class="min-w-0">
        <h1 class="font-heading text-xl font-semibold tracking-tight">{{ title }}</h1>
        <p v-if="description" class="text-muted-foreground text-sm">{{ description }}</p>
      </div>
    </div>
    <div v-if="$slots.actions" class="flex shrink-0 flex-wrap items-center gap-2">
      <slot name="actions" />
    </div>
  </header>
</template>
