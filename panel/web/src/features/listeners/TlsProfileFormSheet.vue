<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { Save } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { TlsProfileView } from '@/api/generated'
import { putTlsProfileMutation } from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
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
import {
  changeHeaders,
  notifyFailure,
  plainHeaders,
  useRefreshConfiguration,
} from '@/lib/configuration'
import {
  ALPN_PROTOCOLS,
  RESOURCE_ID,
  TLS_VERSIONS,
  tlsProfileBody,
  tlsProfileForm,
  type TlsProfileForm,
} from './forms'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ profile?: TlsProfileView; taken: readonly string[] }>()

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const put = useMutation(putTlsProfileMutation())

const form = reactive<TlsProfileForm>(tlsProfileForm())
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, tlsProfileForm(props.profile))
  }
})

const idError = computed(() => {
  if (props.profile || form.id === '') {
    return null
  }
  if (!RESOURCE_ID.test(form.id)) {
    return t('listeners.idHint')
  }
  return props.taken.includes(form.id) ? t('listeners.idTaken') : null
})

function toggleAlpn(protocol: string, checked: boolean | 'indeterminate') {
  form.alpn =
    checked === true
      ? ALPN_PROTOCOLS.filter((value) => value === protocol || form.alpn.includes(value))
      : form.alpn.filter((value) => value !== protocol)
}

function submit() {
  const body = tlsProfileBody(form)
  put.mutate(
    {
      path: { id: body.id },
      body,
      headers: props.profile ? changeHeaders(props.profile.etag) : plainHeaders(),
    },
    {
      onSuccess: (saved) => {
        toast.success(t('listeners.profiles.saved', { id: saved.id }))
        open.value = false
        void refresh()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>{{
            profile ? t('listeners.profiles.edit') : t('listeners.profiles.new')
          }}</SheetTitle>
          <SheetDescription>{{ t('listeners.profiles.emptyDetail') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-4 px-4">
          <FormField
            id="profile-id"
            :label="t('listeners.id')"
            :hint="idError ?? t('listeners.idHint')"
          >
            <Input
              id="profile-id"
              v-model="form.id"
              required
              :disabled="Boolean(profile)"
              :aria-invalid="idError !== null"
              class="font-mono text-xs"
              autocomplete="off"
            />
          </FormField>
          <FormField
            id="profile-certificate"
            :label="t('listeners.profiles.certificate')"
            :hint="t('listeners.profiles.secretHint')"
          >
            <Input
              id="profile-certificate"
              v-model="form.certificateSecretId"
              required
              class="font-mono text-xs"
              placeholder="example.com.crt"
              autocomplete="off"
            />
          </FormField>
          <FormField
            id="profile-key"
            :label="t('listeners.profiles.privateKey')"
            :hint="t('listeners.profiles.secretHint')"
          >
            <Input
              id="profile-key"
              v-model="form.privateKeySecretId"
              required
              class="font-mono text-xs"
              placeholder="example.com.key"
              autocomplete="off"
            />
          </FormField>
          <FormField id="profile-min" :label="t('listeners.profiles.minProtocol')">
            <Select v-model="form.minProtocol">
              <SelectTrigger id="profile-min" class="w-full"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem v-for="version in TLS_VERSIONS" :key="version" :value="version">
                  {{ version }}
                </SelectItem>
              </SelectContent>
            </Select>
          </FormField>
          <fieldset class="flex flex-col gap-2">
            <legend class="mb-1.5 text-sm font-medium">{{ t('listeners.profiles.alpn') }}</legend>
            <label
              v-for="protocol in ALPN_PROTOCOLS"
              :key="protocol"
              class="flex items-center gap-2 text-sm"
            >
              <Checkbox
                :model-value="form.alpn.includes(protocol)"
                @update:model-value="toggleAlpn(protocol, $event)"
              />
              <span class="font-mono text-xs">{{ protocol }}</span>
            </label>
          </fieldset>
        </div>
        <SheetFooter>
          <Button type="submit" :disabled="put.isPending.value || idError !== null">
            <Save data-icon="inline-start" aria-hidden="true" />
            {{ t('common.save') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
