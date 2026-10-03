<script setup lang="ts">
import { Label } from '@/components/ui/label'
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
  <div class="flex items-start justify-between gap-4 rounded-md border p-3">
    <div class="flex flex-col gap-0.5">
      <Label :for="id">{{ label }}</Label>
      <p v-if="hint" class="text-muted-foreground text-xs">{{ hint }}</p>
    </div>
    <Switch :id="id" v-model="model" :disabled="disabled" />
  </div>
</template>
