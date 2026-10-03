<script setup lang="ts">
import type { Component } from 'vue'
import { CircleDashed, CornerLeftUp, PenLine } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { Explanation, SettingSource } from '@/api/generated'
import { Badge } from '@/components/ui/badge'

defineProps<{ explanation: Explanation }>()
const emit = defineEmits<{ select: [span: string] }>()

const { t } = useI18n()

const icons: Record<SettingSource, Component> = {
  here: PenLine,
  inherited: CornerLeftUp,
  default: CircleDashed,
}
</script>

<template>
  <div class="flex flex-col gap-3">
    <p class="flex flex-wrap items-baseline gap-x-2 text-sm">
      <span class="font-medium">{{ t(`studio.block.${explanation.block}`) }}</span>
      <code v-if="explanation.name" class="font-mono">{{ explanation.name }}</code>
      <button
        type="button"
        class="text-muted-foreground font-mono text-xs underline-offset-2 hover:underline focus-visible:underline"
        @click="emit('select', explanation.source_span)"
      >
        {{ explanation.source_span }}
      </button>
    </p>
    <ul class="flex flex-col divide-y" :aria-label="t('studio.effective')">
      <li
        v-for="(setting, index) in explanation.settings"
        :key="`${index}-${setting.name}-${setting.scope ?? ''}`"
        class="flex items-start gap-2 py-2 text-sm first:pt-0 last:pb-0"
      >
        <component
          :is="icons[setting.source] ?? CircleDashed"
          class="mt-0.5 size-4 shrink-0"
          :aria-label="t(`studio.source.${setting.source}`)"
          role="img"
        />
        <div class="min-w-0 flex-1">
          <p class="flex flex-wrap items-center gap-x-2 gap-y-1">
            <code class="font-mono font-semibold">{{ setting.name }}</code>
            <Badge v-if="setting.scope" variant="outline" class="font-mono">
              {{ setting.scope }}
            </Badge>
            <code v-if="setting.value" class="font-mono break-all">{{ setting.value }}</code>
            <span v-else class="text-muted-foreground">{{ t('studio.noValue') }}</span>
          </p>
          <p class="text-muted-foreground flex flex-wrap items-center gap-x-2 text-xs">
            <span>
              {{
                setting.source === 'inherited' && setting.from
                  ? t('studio.inheritedFrom', { block: setting.from })
                  : t(`studio.source.${setting.source}`)
              }}
            </span>
            <button
              v-if="setting.source_span"
              type="button"
              class="font-mono underline-offset-2 hover:underline focus-visible:underline"
              @click="emit('select', setting.source_span)"
            >
              {{ setting.source_span }}
            </button>
          </p>
          <p v-if="setting.rule && setting.source !== 'here'" class="text-muted-foreground text-xs">
            {{ setting.rule }}
          </p>
        </div>
      </li>
    </ul>
  </div>
</template>
