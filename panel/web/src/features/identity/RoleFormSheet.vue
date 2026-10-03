<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Save, ShieldPlus } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { RoleView } from '@/api/generated'
import {
  createRoleMutation,
  permissionsOptions,
  replaceRoleMutation,
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
import { Textarea } from '@/components/ui/textarea'
import { notifyFailure } from '@/lib/configuration'
import { permissionKey } from './presentation'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ role?: RoleView }>()
const emit = defineEmits<{ saved: [] }>()

const { t, te } = useI18n()
const catalog = useQuery(permissionsOptions())
const create = useMutation(createRoleMutation())
const replace = useMutation(replaceRoleMutation())
const busy = computed(() => create.isPending.value || replace.isPending.value)

const form = reactive({ id: '', name: '', description: '', permissions: [] as string[] })
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, {
      id: props.role?.id ?? '',
      name: props.role?.name ?? '',
      description: props.role?.description ?? '',
      permissions: [...(props.role?.permissions ?? [])],
    })
  }
})

function toggle(permission: string, checked: boolean | 'indeterminate') {
  form.permissions = checked
    ? [...new Set([...form.permissions, permission])]
    : form.permissions.filter((item) => item !== permission)
}

function done(message: string) {
  toast.success(message)
  open.value = false
  emit('saved')
}

function submit() {
  const onError = (error: unknown) => notifyFailure(error, t('common.changeFailed'))
  const fields = {
    name: form.name.trim(),
    description: form.description.trim(),
    permissions: form.permissions,
  }
  if (props.role) {
    replace.mutate(
      { path: { id: props.role.id }, body: fields },
      { onSuccess: () => done(t('roles.updated')), onError },
    )
  } else {
    create.mutate(
      { body: { id: form.id, ...fields } },
      { onSuccess: (role) => done(t('roles.created', { name: role.name })), onError },
    )
  }
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>{{ role ? t('roles.edit') : t('roles.new') }}</SheetTitle>
          <SheetDescription>{{ role?.id ?? t('roles.description') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-5 px-4">
          <FormField v-if="!role" id="role-id" :label="t('roles.id')" :hint="t('roles.idHint')">
            <Input
              id="role-id"
              v-model="form.id"
              autocomplete="off"
              autocapitalize="none"
              spellcheck="false"
              pattern="[A-Za-z0-9][A-Za-z0-9._\-]{0,63}"
              required
            />
          </FormField>
          <FormField id="role-name" :label="t('roles.name')">
            <Input id="role-name" v-model="form.name" maxlength="64" required />
          </FormField>
          <FormField id="role-description" :label="t('roles.summary')">
            <Textarea id="role-description" v-model="form.description" maxlength="256" rows="2" />
          </FormField>
          <fieldset class="flex flex-col gap-2">
            <legend class="mb-1 text-sm font-medium">{{ t('roles.permissions') }}</legend>
            <div
              v-for="permission in catalog.data.value ?? []"
              :key="permission.name"
              class="flex items-start gap-2"
            >
              <Checkbox
                :id="`role-permission-${permission.name}`"
                :model-value="form.permissions.includes(permission.name)"
                @update:model-value="toggle(permission.name, $event)"
              />
              <Label
                :for="`role-permission-${permission.name}`"
                class="flex flex-col items-start gap-0.5"
              >
                <code class="font-mono text-xs">{{ permission.name }}</code>
                <span class="text-muted-foreground text-xs font-normal">
                  {{
                    te(permissionKey(permission.name))
                      ? t(permissionKey(permission.name))
                      : permission.description
                  }}
                </span>
              </Label>
            </div>
          </fieldset>
        </div>
        <SheetFooter>
          <Button
            type="submit"
            :disabled="busy || !form.name.trim() || !form.permissions.length || (!role && !form.id)"
          >
            <Spinner v-if="busy" data-icon="inline-start" />
            <Save v-else-if="role" data-icon="inline-start" aria-hidden="true" />
            <ShieldPlus v-else data-icon="inline-start" aria-hidden="true" />
            {{ role ? t('common.save') : t('common.create') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
