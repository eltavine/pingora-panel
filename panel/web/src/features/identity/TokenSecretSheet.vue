<script setup lang="ts">
import { RefreshCw, TriangleAlert } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { CreatedToken } from '@/api/generated'
import CopyValue from '@/components/CopyValue.vue'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'

const open = defineModel<boolean>('open', { required: true })
defineProps<{ token?: CreatedToken }>()
const { t } = useI18n()
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2">
          <RefreshCw class="size-5" aria-hidden="true" />
          {{ t('account.rotated') }}
        </SheetTitle>
        <SheetDescription>{{ token?.token.name }}</SheetDescription>
      </SheetHeader>
      <div v-if="token" class="flex flex-col gap-4 px-4">
        <Alert>
          <TriangleAlert aria-hidden="true" />
          <AlertDescription>{{ t('account.rotatedDetail') }}</AlertDescription>
        </Alert>
        <CopyValue :value="token.secret" />
      </div>
      <SheetFooter>
        <Button @click="open = false">{{ t('account.done') }}</Button>
      </SheetFooter>
    </SheetContent>
  </Sheet>
</template>
