<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { LockKeyhole } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { AccountView } from '@/api/generated'
import { resetPasswordMutation } from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Spinner } from '@/components/ui/spinner'
import { notifyFailure } from '@/lib/configuration'
import { fieldProblems } from './failures'
import PasswordInput from './PasswordInput.vue'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ account: AccountView }>()
const emit = defineEmits<{ saved: [] }>()

const { t } = useI18n()
const reset = useMutation(resetPasswordMutation())
const password = ref('')
const confirmation = ref('')
const problems = ref<string[]>([])
const mismatch = computed(() => confirmation.value !== '' && confirmation.value !== password.value)

watch(open, (isOpen) => {
  if (isOpen) {
    password.value = ''
    confirmation.value = ''
    problems.value = []
  }
})

function submit() {
  problems.value = []
  reset.mutate(
    { path: { id: props.account.id }, body: { password: password.value } },
    {
      onSuccess: () => {
        toast.success(t('accounts.passwordReset'))
        open.value = false
        emit('saved')
      },
      onError: (error) => {
        problems.value = fieldProblems(error, 'password')
        if (!problems.value.length) {
          notifyFailure(error, t('common.changeFailed'))
        }
      },
    },
  )
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>{{ t('accounts.resetTitle', { name: account.username }) }}</SheetTitle>
          <SheetDescription>{{ t('auth.passwordHint') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-5 px-4">
          <FormField id="reset-password" :label="t('account.newPassword')">
            <PasswordInput
              id="reset-password"
              v-model="password"
              autocomplete="new-password"
              :invalid="problems.length > 0"
            />
          </FormField>
          <FormField id="reset-confirm" :label="t('account.confirmPassword')">
            <PasswordInput
              id="reset-confirm"
              v-model="confirmation"
              autocomplete="new-password"
              :invalid="mismatch"
            />
          </FormField>
          <ul
            v-if="problems.length || mismatch"
            class="text-destructive flex flex-col gap-0.5 text-xs"
            role="alert"
          >
            <li v-if="mismatch">{{ t('auth.mismatch') }}</li>
            <li v-for="item in problems" :key="item">{{ item }}</li>
          </ul>
        </div>
        <SheetFooter>
          <Button type="submit" :disabled="reset.isPending.value || mismatch || !password">
            <Spinner v-if="reset.isPending.value" data-icon="inline-start" />
            <LockKeyhole v-else data-icon="inline-start" aria-hidden="true" />
            {{ t('accounts.resetPassword') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
