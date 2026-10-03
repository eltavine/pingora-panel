<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{
  /** A unified diff. */
  diff: string
}>()

type Kind = 'file' | 'hunk' | 'added' | 'removed' | 'context'

// Lines are told apart by their marker and weight, not by hue.
const lines = computed(() =>
  props.diff
    .replace(/\n$/, '')
    .split('\n')
    .map((text) => {
      const kind: Kind =
        text.startsWith('+++') || text.startsWith('---')
          ? 'file'
          : text.startsWith('@@')
            ? 'hunk'
            : text.startsWith('+')
              ? 'added'
              : text.startsWith('-')
                ? 'removed'
                : 'context'
      return { kind, text }
    }),
)
</script>

<template>
  <pre
    class="bg-background overflow-x-auto rounded-md border py-2 font-mono text-xs leading-relaxed"
  ><span
      v-for="(line, index) in lines"
      :key="index"
      :data-kind="line.kind"
      class="block min-w-max px-3 whitespace-pre"
      :class="{
        'font-semibold': line.kind === 'file' || line.kind === 'added',
        'text-muted-foreground': line.kind === 'hunk' || line.kind === 'removed',
        'bg-foreground/[0.07]': line.kind === 'added',
        'bg-foreground/[0.025]': line.kind === 'removed',
      }"
    >{{ line.text || ' ' }}</span></pre>
</template>
