<script setup lang="ts">
import type { Component } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'

const open = defineModel<boolean>('open', { required: true })

defineProps<{
  icon: Component
  title: string
  description: string
  confirmLabel: string
  destructive?: boolean
  busy?: boolean
}>()

const emit = defineEmits<{ confirm: [] }>()
const { t } = useI18n()

// The dialog closes only after `confirm` is handled: closing first would
// clear what callers keep about the item being confirmed.
function confirm() {
  emit('confirm')
  open.value = false
}
</script>

<template>
  <AlertDialog v-model:open="open">
    <AlertDialogContent>
      <AlertDialogHeader>
        <AlertDialogTitle class="flex items-center gap-2">
          <component :is="icon" class="size-5" aria-hidden="true" />
          {{ title }}
        </AlertDialogTitle>
        <AlertDialogDescription>{{ description }}</AlertDialogDescription>
      </AlertDialogHeader>
      <slot />
      <AlertDialogFooter>
        <AlertDialogCancel>{{ t('common.cancel') }}</AlertDialogCancel>
        <Button
          :variant="destructive ? 'destructive' : 'default'"
          :disabled="busy"
          @click="confirm"
        >
          {{ confirmLabel }}
        </Button>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>
</template>
