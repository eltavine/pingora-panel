<script setup lang="ts">
import { useClipboard } from '@vueuse/core'
import { Check, Copy } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { Button } from '@/components/ui/button'

const props = defineProps<{
  value: string
}>()

const { t } = useI18n()
const { copy, copied, isSupported } = useClipboard({ copiedDuring: 1500 })
</script>

<template>
  <span class="inline-flex min-w-0 max-w-full items-center gap-1">
    <code
      class="bg-muted min-w-0 truncate rounded px-1.5 py-0.5 font-mono text-xs"
      :title="props.value"
      >{{ props.value }}</code
    >
    <Button
      v-if="isSupported"
      variant="ghost"
      size="icon-xs"
      :aria-label="copied ? t('state.copied') : t('state.copy')"
      @click="copy(props.value)"
    >
      <Check v-if="copied" aria-hidden="true" />
      <Copy v-else aria-hidden="true" />
    </Button>
  </span>
</template>
