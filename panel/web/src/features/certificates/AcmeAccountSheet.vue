<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { ExternalLink, UserRoundKey } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { NewAcmeAccount } from '@/api/generated'
import { createAcmeAccountMutation } from '@/api/generated/@tanstack/vue-query.gen'
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
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { ACCOUNT_ID, ACME_DIRECTORIES, parseNames } from './presentation'

const CUSTOM = 'custom'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ taken: readonly string[] }>()
const emit = defineEmits<{ saved: [] }>()

const { t } = useI18n()
const create = useMutation(createAcmeAccountMutation())

function blank() {
  return {
    ca: 'letsencrypt',
    url: '',
    caBundle: '',
    id: 'letsencrypt',
    idEdited: false,
    emails: '',
    binding: false,
    keyId: '',
    macKey: '',
    agreed: false,
  }
}
const form = reactive(blank())
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, blank())
  }
})

const preset = computed(() => ACME_DIRECTORIES.find((directory) => directory.id === form.ca))
const needsBinding = computed(() => preset.value?.binding ?? false)
const usesBinding = computed(() => needsBinding.value || form.binding)
watch(
  () => form.ca,
  (ca) => {
    if (!form.idEdited) {
      form.id = ca === CUSTOM ? '' : ca
    }
  },
)

const idError = computed(() => {
  if (form.id === '') {
    return null
  }
  if (!ACCOUNT_ID.test(form.id)) {
    return t('certificates.acme.accountIdHint')
  }
  return props.taken.includes(form.id) ? t('certificates.acme.accountIdTaken') : null
})
const directory = computed(() => preset.value?.url ?? form.url.trim())
const directoryValid = computed(() => /^https:\/\/[^\s/?#]+/.test(directory.value))
const bindingValid = computed(
  () => !usesBinding.value || (form.keyId.trim() !== '' && form.macKey.trim() !== ''),
)
const ready = computed(
  () =>
    form.id !== '' &&
    idError.value === null &&
    directoryValid.value &&
    bindingValid.value &&
    form.agreed,
)

function submit() {
  const body: NewAcmeAccount = {
    id: form.id,
    directory: directory.value,
    contact: parseNames(form.emails),
    terms_of_service_agreed: form.agreed,
  }
  if (form.ca === CUSTOM && form.caBundle.trim() !== '') {
    body.ca_bundle = form.caBundle
  }
  if (usesBinding.value) {
    body.external_account = { key_id: form.keyId.trim(), mac_key: form.macKey.trim() }
  }
  create.mutate(
    { body, headers: plainHeaders() },
    {
      onSuccess: (created) => {
        toast.success(t('certificates.acme.registered', { id: created.id }))
        open.value = false
        emit('saved')
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
          <SheetTitle class="flex items-center gap-2">
            <UserRoundKey class="size-5" aria-hidden="true" />
            {{ t('certificates.acme.register') }}
          </SheetTitle>
          <SheetDescription>{{ t('certificates.acme.registerDetail') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-4 px-4">
          <FormField id="acme-ca" :label="t('certificates.acme.ca')">
            <Select v-model="form.ca">
              <SelectTrigger id="acme-ca" class="w-full"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem
                  v-for="candidate in ACME_DIRECTORIES"
                  :key="candidate.id"
                  :value="candidate.id"
                >
                  {{ t(`certificates.acme.directories.${candidate.id}`) }}
                </SelectItem>
                <SelectItem :value="CUSTOM">{{
                  t('certificates.acme.directories.custom')
                }}</SelectItem>
              </SelectContent>
            </Select>
          </FormField>
          <template v-if="form.ca === CUSTOM">
            <FormField
              id="acme-directory"
              :label="t('certificates.acme.directory')"
              :hint="t('certificates.acme.directoryHint')"
            >
              <Input
                id="acme-directory"
                v-model="form.url"
                type="url"
                required
                class="font-mono text-xs"
                placeholder="https://ca.example/acme/directory"
                autocomplete="off"
              />
            </FormField>
            <FormField
              id="acme-ca-bundle"
              :label="t('certificates.acme.caBundle')"
              :hint="t('certificates.acme.caBundleHint')"
            >
              <Textarea
                id="acme-ca-bundle"
                v-model="form.caBundle"
                rows="4"
                spellcheck="false"
                class="font-mono text-xs"
                placeholder="-----BEGIN CERTIFICATE-----"
              />
            </FormField>
          </template>
          <FormField
            id="acme-account-id"
            :label="t('certificates.acme.accountId')"
            :hint="idError ?? t('certificates.acme.accountIdHint')"
          >
            <Input
              id="acme-account-id"
              v-model="form.id"
              required
              :aria-invalid="idError !== null"
              class="font-mono text-xs"
              autocomplete="off"
              @input="form.idEdited = true"
            />
          </FormField>
          <FormField
            id="acme-emails"
            :label="t('certificates.acme.emails')"
            :hint="t('certificates.acme.emailsHint')"
          >
            <Input
              id="acme-emails"
              v-model="form.emails"
              class="text-sm"
              placeholder="ops@example.com"
              autocomplete="email"
            />
          </FormField>
          <div class="flex flex-col gap-4 rounded-lg border p-3">
            <div class="flex items-center justify-between gap-3">
              <div class="flex flex-col gap-0.5">
                <span class="text-sm font-medium" id="acme-binding-label">{{
                  t('certificates.acme.binding')
                }}</span>
                <span class="text-muted-foreground text-xs">{{
                  needsBinding
                    ? t('certificates.acme.bindingRequired')
                    : t('certificates.acme.bindingHint')
                }}</span>
              </div>
              <Switch
                v-if="!needsBinding"
                v-model="form.binding"
                aria-labelledby="acme-binding-label"
              />
            </div>
            <template v-if="usesBinding">
              <FormField id="acme-key-id" :label="t('certificates.acme.keyId')">
                <Input
                  id="acme-key-id"
                  v-model="form.keyId"
                  required
                  class="font-mono text-xs"
                  autocomplete="off"
                />
              </FormField>
              <FormField
                id="acme-mac-key"
                :label="t('certificates.acme.macKey')"
                :hint="t('certificates.acme.macKeyHint')"
              >
                <Input
                  id="acme-mac-key"
                  v-model="form.macKey"
                  type="password"
                  required
                  class="font-mono text-xs"
                  autocomplete="off"
                />
              </FormField>
            </template>
          </div>
          <label class="flex items-start gap-3 text-sm">
            <Checkbox v-model="form.agreed" class="mt-0.5" />
            <span class="flex flex-col gap-1">
              {{ t('certificates.acme.agree') }}
              <a
                v-if="preset"
                :href="preset.terms"
                target="_blank"
                rel="noopener noreferrer"
                class="text-muted-foreground inline-flex items-center gap-1 text-xs underline-offset-4 hover:underline"
              >
                {{ t('certificates.acme.terms') }}
                <ExternalLink class="size-3" aria-hidden="true" />
              </a>
            </span>
          </label>
        </div>
        <SheetFooter>
          <Button type="submit" :disabled="create.isPending.value || !ready">
            <UserRoundKey data-icon="inline-start" aria-hidden="true" />
            {{ t('certificates.acme.register') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
