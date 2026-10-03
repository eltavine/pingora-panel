<script setup lang="ts">
import type { Component } from 'vue'
import { CircleX, Info, TriangleAlert } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { DiagnosticDetails, Severity } from '@/api/generated'

defineProps<{
  diagnostics: readonly DiagnosticDetails[]
  /** Places become buttons that select the diagnostic. */
  selectable?: boolean
}>()

const emit = defineEmits<{ select: [diagnostic: DiagnosticDetails] }>()

const { t } = useI18n()

const icons: Record<Severity, Component> = {
  ERROR: CircleX,
  WARNING: TriangleAlert,
  INFO: Info,
}
</script>

<template>
  <ul class="flex flex-col divide-y">
    <li
      v-for="(item, index) in diagnostics"
      :key="`${index}-${item.code}-${item.source_span ?? item.resource_id ?? ''}`"
      class="flex items-start gap-2 py-2 text-sm first:pt-0 last:pb-0"
    >
      <component
        :is="icons[item.severity] ?? CircleX"
        class="mt-0.5 size-4 shrink-0"
        :aria-label="t(`diagnostics.severity.${item.severity}`)"
        role="img"
      />
      <div class="min-w-0 flex-1">
        <p class="break-words">{{ item.message }}</p>
        <p class="text-muted-foreground flex flex-wrap items-center gap-x-2 text-xs">
          <button
            v-if="selectable && item.source_span"
            type="button"
            class="font-mono underline-offset-2 hover:underline focus-visible:underline"
            @click="emit('select', item)"
          >
            {{ item.source_span }}
          </button>
          <span v-else-if="item.source_span ?? item.resource_id" class="font-mono">
            {{ item.source_span ?? item.resource_id }}
          </span>
          <code class="font-mono">{{ item.code }}</code>
        </p>
        <p v-if="item.help" class="text-muted-foreground text-xs">{{ item.help }}</p>
      </div>
    </li>
  </ul>
</template>
