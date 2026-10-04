<script setup lang="ts">
import { KeyRound } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { AlertChannelSecretView } from '@/api/generated'
import CopyValue from '@/components/CopyValue.vue'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'

const secret = defineModel<AlertChannelSecretView | undefined>('secret', { required: true })
const { t } = useI18n()

function close(open: boolean) {
  if (!open) {
    secret.value = undefined
  }
}
</script>

<template>
  <AlertDialog :open="secret !== undefined" @update:open="close">
    <AlertDialogContent>
      <AlertDialogHeader>
        <AlertDialogTitle class="flex items-center gap-2">
          <KeyRound class="size-5" aria-hidden="true" />
          {{ t('alerts.secretTitle', { id: secret?.channel.id ?? '' }) }}
        </AlertDialogTitle>
        <AlertDialogDescription>{{ t('alerts.secretDescription') }}</AlertDialogDescription>
      </AlertDialogHeader>
      <CopyValue v-if="secret" :value="secret.secret" />
      <AlertDialogFooter>
        <AlertDialogAction>{{ t('alerts.secretStored') }}</AlertDialogAction>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>
</template>
