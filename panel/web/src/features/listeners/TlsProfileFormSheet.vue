<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { FileBadge, FolderKey, Save } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { TlsProfileView } from '@/api/generated'
import {
  listCertificatesOptions,
  putTlsProfileMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import SwitchField from '@/components/SwitchField.vue'
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import {
  changeHeaders,
  notifyFailure,
  plainHeaders,
  useRefreshConfiguration,
} from '@/lib/configuration'
import { useSession } from '@/lib/session'
import {
  ALPN_PROTOCOLS,
  CIPHER_SUITES,
  NEWEST,
  RESOURCE_ID,
  TLS_VERSIONS,
  tlsProfileBody,
  tlsProfileForm,
  type TlsProfileForm,
} from './forms'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ profile?: TlsProfileView; taken: readonly string[] }>()

const { t } = useI18n()
const { can } = useSession()
const refresh = useRefreshConfiguration()
const put = useMutation(putTlsProfileMutation())
const canReadCertificates = computed(() => can('certificate.read'))
const certificates = useQuery({ ...listCertificatesOptions(), enabled: canReadCertificates })

const form = reactive<TlsProfileForm>(tlsProfileForm())
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, tlsProfileForm(props.profile))
    if (!props.profile && !canReadCertificates.value) {
      form.source = 'files'
    }
  }
})
const certificateMissing = computed(() => form.source === 'inventory' && !form.certificateId)

const idError = computed(() => {
  if (props.profile || form.id === '') {
    return null
  }
  if (!RESOURCE_ID.test(form.id)) {
    return t('listeners.idHint')
  }
  return props.taken.includes(form.id) ? t('listeners.idTaken') : null
})

function toggleSuite(suite: string, checked: boolean | 'indeterminate') {
  form.cipherSuites =
    checked === true
      ? [...form.cipherSuites, suite]
      : form.cipherSuites.filter((value) => value !== suite)
}

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
          <Tabs v-model="form.source" class="gap-3">
            <TabsList class="w-full" :aria-label="t('listeners.profiles.source')">
              <TabsTrigger value="inventory" :disabled="!canReadCertificates">
                <FileBadge aria-hidden="true" />
                {{ t('listeners.profiles.fromInventory') }}
              </TabsTrigger>
              <TabsTrigger value="files">
                <FolderKey aria-hidden="true" />
                {{ t('listeners.profiles.fromFiles') }}
              </TabsTrigger>
            </TabsList>
            <TabsContent value="inventory">
              <FormField
                id="profile-certificate-id"
                :label="t('listeners.profiles.inventoryCertificate')"
                :hint="
                  certificates.data.value?.length === 0
                    ? t('listeners.profiles.noCertificates')
                    : t('listeners.profiles.inventoryHint')
                "
              >
                <Select v-model="form.certificateId">
                  <SelectTrigger id="profile-certificate-id" class="w-full">
                    <SelectValue :placeholder="t('listeners.profiles.pickCertificate')" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem
                      v-for="certificate in certificates.data.value"
                      :key="certificate.id"
                      :value="certificate.id"
                    >
                      <span class="font-mono text-xs">{{ certificate.id }}</span>
                      <span class="text-muted-foreground truncate text-xs">{{
                        ` · ${certificate.names.join(', ')}`
                      }}</span>
                    </SelectItem>
                  </SelectContent>
                </Select>
              </FormField>
            </TabsContent>
            <TabsContent value="files" class="flex flex-col gap-4">
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
            </TabsContent>
          </Tabs>
          <div class="grid gap-4 sm:grid-cols-2">
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
            <FormField id="profile-max" :label="t('listeners.profiles.maxProtocol')">
              <Select v-model="form.maxProtocol">
                <SelectTrigger id="profile-max" class="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem :value="NEWEST">{{ t('listeners.profiles.newest') }}</SelectItem>
                  <SelectItem v-for="version in TLS_VERSIONS" :key="version" :value="version">
                    {{ version }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </FormField>
          </div>
          <fieldset class="flex flex-col gap-2">
            <legend class="mb-1.5 text-sm font-medium">
              {{ t('listeners.profiles.ciphers') }}
            </legend>
            <template v-for="(suites, version) in CIPHER_SUITES" :key="version">
              <span class="text-muted-foreground text-xs font-medium">{{ version }}</span>
              <label v-for="suite in suites" :key="suite" class="flex items-center gap-2 text-sm">
                <Checkbox
                  :model-value="form.cipherSuites.includes(suite)"
                  @update:model-value="toggleSuite(suite, $event)"
                />
                <span class="font-mono text-xs break-all">{{ suite }}</span>
              </label>
            </template>
            <p class="text-muted-foreground text-xs">{{ t('listeners.profiles.ciphersHint') }}</p>
          </fieldset>
          <SwitchField
            id="profile-resumption"
            v-model="form.sessionResumption"
            :label="t('listeners.profiles.sessionResumption')"
            :hint="t('listeners.profiles.sessionResumptionHint')"
          />
          <SwitchField
            id="profile-ocsp"
            v-model="form.ocspStapling"
            :label="t('listeners.profiles.ocspStapling')"
            :hint="t('listeners.profiles.ocspReserved')"
          />
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
          <Button
            type="submit"
            :disabled="put.isPending.value || idError !== null || certificateMissing"
          >
            <Save data-icon="inline-start" aria-hidden="true" />
            {{ t('common.save') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
