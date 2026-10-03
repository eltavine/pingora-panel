<script setup lang="ts">
import { Braces, CornerDownRight } from '@lucide/vue'
import type { SyntaxNode } from '@/api/generated'

defineProps<{ nodes: readonly SyntaxNode[] }>()
const emit = defineEmits<{ select: [node: SyntaxNode] }>()
</script>

<template>
  <ul class="flex flex-col">
    <li v-for="node in nodes" :key="node.span">
      <button
        type="button"
        class="hover:bg-muted flex w-full min-w-0 items-center gap-1.5 rounded px-1.5 py-0.5 text-left font-mono text-xs"
        :title="[...(node.comments ?? []), node.span].join('\n')"
        @click="emit('select', node)"
      >
        <Braces v-if="node.block" class="size-3.5 shrink-0" aria-hidden="true" />
        <CornerDownRight
          v-else
          class="text-muted-foreground size-3.5 shrink-0"
          aria-hidden="true"
        />
        <span class="font-semibold">{{ node.name }}</span>
        <span class="text-muted-foreground truncate">{{ node.args.join(' ') }}</span>
      </button>
      <OutlineTree
        v-if="node.block?.length"
        :nodes="node.block"
        class="border-border ml-3 border-l pl-1.5"
        @select="emit('select', $event)"
      />
    </li>
  </ul>
</template>
