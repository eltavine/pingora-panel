<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/utils'

const props = defineProps<{
  class?: HTMLAttributes['class']
}>()
</script>

<template>
  <div
    data-slot="sidebar-content"
    data-sidebar="content"
    :class="
      cn(
        'gap-2 flex min-h-0 flex-1 flex-col overflow-auto group-data-[collapsible=icon]:overflow-hidden',
        props.class,
      )
    "
  >
    <slot />
  </div>
</template>

<style scoped>
[data-sidebar='content'] {
  --scrollbar-thumb: color-mix(in oklch, var(--sidebar-ring) 80%, transparent);
  scrollbar-width: thin;
  scrollbar-color: var(--scrollbar-thumb) transparent;
}

[data-sidebar='content']:hover,
[data-sidebar='content']:focus-within {
  --scrollbar-thumb: var(--sidebar-ring);
}

@supports selector(::-webkit-scrollbar) {
  [data-sidebar='content'] {
    scrollbar-width: auto;
    scrollbar-color: auto;
  }

  [data-sidebar='content']::-webkit-scrollbar {
    width: 8px;
    height: 8px;
  }

  [data-sidebar='content']::-webkit-scrollbar-track,
  [data-sidebar='content']::-webkit-scrollbar-corner {
    background: transparent;
  }

  [data-sidebar='content']::-webkit-scrollbar-thumb {
    min-height: 32px;
    border: 2px solid transparent;
    border-radius: 999px;
    background: var(--scrollbar-thumb);
    background-clip: padding-box;
  }

  [data-sidebar='content']::-webkit-scrollbar-thumb:active {
    background-color: var(--sidebar-foreground);
  }

  [data-sidebar='content']::-webkit-scrollbar-button {
    display: none;
  }
}
</style>
