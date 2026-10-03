<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Save, UserPlus } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { AccountView } from '@/api/generated'
import {
  createAccountMutation,
  listRolesOptions,
  updateAccountMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
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
const props = defineProps<{ account?: AccountView }>()
const emit = defineEmits<{ saved: [] }>()

const { t } = useI18n()
const roles = useQuery(listRolesOptions())
const create = useMutation(createAccountMutation())
const update = useMutation(updateAccountMutation())
const busy = computed(() => create.isPending.value || update.isPending.value)

const form = reactive({ username: '', displayName: '', password: '', roles: [] as string[] })
const passwordProblems = ref<string[]>([])
watch(open, (isOpen) => {
  if (isOpen) {
    passwordProblems.value = []
    Object.assign(form, {
      username: props.account?.username ?? '',
      displayName: props.account?.display_name ?? '',
      password: '',
      roles: [...(props.account?.roles ?? ['viewer'])],
    })
  }
})

function toggle(role: string, checked: boolean | 'indeterminate') {
  form.roles = checked
    ? [...new Set([...form.roles, role])]
    : form.roles.filter((item) => item !== role)
}

function done(message: string) {
  toast.success(message)
  open.value = false
  emit('saved')
}

function failed(error: unknown) {
  passwordProblems.value = fieldProblems(error, 'password')
  if (!passwordProblems.value.length) {
    notifyFailure(error, t('common.changeFailed'))
  }
}

function submit() {
  if (props.account) {
    update.mutate(
      {
        path: { id: props.account.id },
        body: { display_name: form.displayName.trim(), roles: form.roles },
      },
      { onSuccess: () => done(t('accounts.updated')), onError: failed },
    )
  } else {
    create.mutate(
      {
        body: {
          username: form.username,
          display_name: form.displayName.trim() || null,
          password: form.password || null,
          roles: form.roles,
        },
      },
      {
        onSuccess: (account) => done(t('accounts.created', { name: account.username })),
        onError: failed,
      },
    )
  }
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>{{ account ? t('accounts.edit') : t('accounts.new') }}</SheetTitle>
          <SheetDescription>{{ account?.username ?? t('accounts.description') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-5 px-4">
          <FormField
            v-if="!account"
            id="account-username"
            :label="t('accounts.username')"
            :hint="t('accounts.usernameHint')"
          >
            <Input
              id="account-username"
              v-model="form.username"
              autocomplete="off"
              autocapitalize="none"
              spellcheck="false"
              pattern="[A-Za-z0-9][A-Za-z0-9._\-]{0,63}"
              required
            />
          </FormField>
          <FormField id="account-display-name" :label="t('accounts.displayName')">
            <Input id="account-display-name" v-model="form.displayName" autocomplete="off" />
          </FormField>
          <fieldset class="flex flex-col gap-2">
            <legend class="mb-1 text-sm font-medium">{{ t('accounts.roles') }}</legend>
            <p class="text-muted-foreground -mt-1 mb-1 text-xs">{{ t('accounts.rolesHint') }}</p>
            <div
              v-for="role in roles.data.value ?? []"
              :key="role.id"
              class="flex items-start gap-2"
            >
              <Checkbox
                :id="`role-${role.id}`"
                :model-value="form.roles.includes(role.id)"
                @update:model-value="toggle(role.id, $event)"
              />
              <Label :for="`role-${role.id}`" class="flex flex-col items-start gap-0.5">
                <span>{{ role.name }}</span>
                <span class="text-muted-foreground text-xs font-normal">{{
                  role.description
                }}</span>
              </Label>
            </div>
          </fieldset>
          <FormField
            v-if="!account"
            id="account-password"
            :label="t('accounts.password')"
            :hint="t('accounts.passwordHint')"
          >
            <PasswordInput
              id="account-password"
              v-model="form.password"
              autocomplete="new-password"
              :required="false"
              :invalid="passwordProblems.length > 0"
            />
            <ul
              v-if="passwordProblems.length"
              class="text-destructive flex flex-col gap-0.5 text-xs"
              role="alert"
            >
              <li v-for="item in passwordProblems" :key="item">{{ item }}</li>
            </ul>
          </FormField>
        </div>
        <SheetFooter>
          <Button type="submit" :disabled="busy || (!account && !form.username)">
            <Spinner v-if="busy" data-icon="inline-start" />
            <Save v-else-if="account" data-icon="inline-start" aria-hidden="true" />
            <UserPlus v-else data-icon="inline-start" aria-hidden="true" />
            {{ account ? t('common.save') : t('common.create') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
