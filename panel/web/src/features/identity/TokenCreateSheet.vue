<script setup lang="ts">
import { reactive, ref, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { KeyRound, TriangleAlert } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { CreatedToken } from '@/api/generated'
import { createTokenMutation } from '@/api/generated/@tanstack/vue-query.gen'
import CopyValue from '@/components/CopyValue.vue'
import FormField from '@/components/FormField.vue'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
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
import { permissionKey, TOKEN_LIFETIMES } from './presentation'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ permissions: readonly string[] }>()
const emit = defineEmits<{ created: [] }>()

const { t, te } = useI18n()
const create = useMutation(createTokenMutation())
const form = reactive({ name: '', days: '90', permissions: [] as string[] })
const created = ref<CreatedToken>()

watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, { name: '', days: '90', permissions: [...props.permissions] })
    created.value = undefined
  }
})

function toggle(permission: string, checked: boolean | 'indeterminate') {
  form.permissions = checked
    ? [...new Set([...form.permissions, permission])]
    : form.permissions.filter((item) => item !== permission)
}

function submit() {
  create.mutate(
    {
      body: {
        name: form.name.trim(),
        permissions: form.permissions,
        expires_in_days: Number(form.days),
      },
    },
    {
      onSuccess: (token) => {
        created.value = token
        emit('created')
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2">
          <KeyRound class="size-5" aria-hidden="true" />
          {{ t('account.newToken') }}
        </SheetTitle>
        <SheetDescription>{{ t('account.tokensDetail') }}</SheetDescription>
      </SheetHeader>

      <div v-if="created" class="flex flex-col gap-4 px-4">
        <Alert>
          <TriangleAlert aria-hidden="true" />
          <AlertDescription>{{ t('account.tokenSecret') }}</AlertDescription>
        </Alert>
        <CopyValue :value="created.secret" />
        <SheetFooter class="px-0">
          <Button @click="open = false">{{ t('account.done') }}</Button>
        </SheetFooter>
      </div>

      <form v-else class="flex flex-col gap-6 px-4" @submit.prevent="submit">
        <FormField id="token-name" :label="t('account.tokenName')">
          <Input id="token-name" v-model="form.name" maxlength="64" required />
        </FormField>
        <FormField id="token-days" :label="t('account.tokenExpiry')">
          <Select v-model="form.days">
            <SelectTrigger id="token-days" class="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="days in TOKEN_LIFETIMES" :key="days" :value="String(days)">
                {{ t('account.days', { count: days }) }}
              </SelectItem>
            </SelectContent>
          </Select>
        </FormField>
        <fieldset class="flex flex-col gap-2">
          <legend class="mb-1 text-sm font-medium">{{ t('account.tokenPermissions') }}</legend>
          <div v-for="permission in permissions" :key="permission" class="flex items-start gap-2">
            <Checkbox
              :id="`token-${permission}`"
              :model-value="form.permissions.includes(permission)"
              @update:model-value="toggle(permission, $event)"
            />
            <Label :for="`token-${permission}`" class="flex flex-col items-start gap-0.5">
              <code class="font-mono text-xs">{{ permission }}</code>
              <span v-if="te(permissionKey(permission))" class="text-muted-foreground text-xs">
                {{ t(permissionKey(permission)) }}
              </span>
            </Label>
          </div>
        </fieldset>
        <SheetFooter class="px-0">
          <Button
            type="submit"
            :disabled="create.isPending.value || !form.name.trim() || !form.permissions.length"
          >
            <Spinner v-if="create.isPending.value" data-icon="inline-start" />
            <KeyRound v-else data-icon="inline-start" aria-hidden="true" />
            {{ t('common.create') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
