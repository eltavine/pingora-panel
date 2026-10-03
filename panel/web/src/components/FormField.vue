<script setup lang="ts">
import { Label } from '@/components/ui/label'

defineProps<{
  id: string
  label: string
  hint?: string
}>()
</script>

<template>
  <div class="flex flex-col gap-1.5">
    <Label :for="id">{{ label }}</Label>
    <slot />
    <p v-if="hint" :id="`${id}-hint`" class="text-muted-foreground text-xs">{{ hint }}</p>
  </div>
</template>
