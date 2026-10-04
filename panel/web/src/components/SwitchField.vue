<script setup lang="ts">
import { Switch } from '@/components/ui/switch'

const model = defineModel<boolean>({ required: true })

defineProps<{
  id: string
  label: string
  hint?: string
  disabled?: boolean
}>()
</script>

<template>
  <label :for="id" class="flex items-start justify-between gap-4 rounded-md border p-3">
    <span class="flex flex-col gap-0.5">
      <span class="text-sm leading-none font-medium select-none">{{ label }}</span>
      <span v-if="hint" class="text-muted-foreground text-xs">{{ hint }}</span>
    </span>
    <Switch :id="id" v-model="model" :disabled="disabled" />
  </label>
</template>
