<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { CalendarSync, Globe, Info, Network } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { AcmeAccountView, AcmeChallenge, DnsProviderView } from '@/api/generated'
import { createAutomaticCertificateMutation } from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
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
import { Textarea } from '@/components/ui/textarea'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { CERTIFICATE_ID, directoryHost, parseNames, suggestedId } from './presentation'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{
  accounts: readonly AcmeAccountView[]
  providers: readonly DnsProviderView[]
  /** IDs of automatic certificates already there. */
  taken: readonly string[]
}>()
const emit = defineEmits<{ saved: [] }>()

const { t } = useI18n()
const create = useMutation(createAutomaticCertificateMutation())

const form = reactive({
  account: '',
  names: '',
  id: '',
  idEdited: false,
  challenge: 'http-01' as AcmeChallenge,
  provider: '',
})
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, {
      account: props.accounts[0]?.id ?? '',
      names: '',
      id: '',
      idEdited: false,
      challenge: 'http-01',
      provider: props.providers[0]?.id ?? '',
    })
  }
})
const names = computed(() => parseNames(form.names))
watch(names, (current) => {
  if (!form.idEdited) {
    form.id = suggestedId(current)
  }
})

const namesError = computed(() =>
  form.challenge === 'http-01' && names.value.some((name) => name.startsWith('*.'))
    ? t('certificates.acme.wildcardNeedsDns')
    : null,
)
const idError = computed(() => {
  if (form.id === '') {
    return null
  }
  if (!CERTIFICATE_ID.test(form.id)) {
    return t('certificates.idHint')
  }
  return props.taken.includes(form.id) ? t('certificates.acme.alreadyAutomatic') : null
})
const ready = computed(
  () =>
    form.account !== '' &&
    names.value.length > 0 &&
    namesError.value === null &&
    form.id !== '' &&
    idError.value === null &&
    (form.challenge === 'http-01' || form.provider !== ''),
)

function submit() {
  create.mutate(
    {
      body: {
        id: form.id,
        account: form.account,
        names: names.value,
        challenge: form.challenge,
        ...(form.challenge === 'dns-01' ? { dns_provider: form.provider } : {}),
      },
      headers: plainHeaders(),
    },
    {
      onSuccess: (created) => {
        toast.success(t('certificates.acme.requested', { id: created.id }))
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
            <CalendarSync class="size-5" aria-hidden="true" />
            {{ t('certificates.acme.request') }}
          </SheetTitle>
          <SheetDescription>{{ t('certificates.acme.requestDetail') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-4 px-4">
          <FormField id="automatic-account" :label="t('certificates.acme.account')">
            <Select v-model="form.account">
              <SelectTrigger id="automatic-account" class="w-full">
                <SelectValue :placeholder="t('certificates.acme.pickAccount')" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem v-for="account in accounts" :key="account.id" :value="account.id">
                  <span class="font-mono text-xs">{{ account.id }}</span>
                  <span class="text-muted-foreground truncate text-xs">{{
                    ` · ${directoryHost(account.directory)}`
                  }}</span>
                </SelectItem>
              </SelectContent>
            </Select>
          </FormField>
          <FormField
            id="automatic-names"
            :label="t('certificates.names')"
            :hint="namesError ?? t('certificates.acme.namesHint')"
          >
            <Textarea
              id="automatic-names"
              v-model="form.names"
              required
              rows="4"
              spellcheck="false"
              :aria-invalid="namesError !== null"
              class="font-mono text-xs"
              placeholder="example.com&#10;www.example.com"
            />
          </FormField>
          <FormField
            id="automatic-id"
            :label="t('certificates.id')"
            :hint="idError ?? t('certificates.acme.idHint')"
          >
            <Input
              id="automatic-id"
              v-model="form.id"
              required
              :aria-invalid="idError !== null"
              class="font-mono text-xs"
              autocomplete="off"
              @input="form.idEdited = true"
            />
          </FormField>
          <div class="flex flex-col gap-2">
            <span class="text-sm font-medium">{{ t('certificates.acme.challenge') }}</span>
            <Tabs v-model="form.challenge">
              <TabsList class="w-full" :aria-label="t('certificates.acme.challenge')">
                <TabsTrigger value="http-01">
                  <Globe aria-hidden="true" />
                  HTTP-01
                </TabsTrigger>
                <TabsTrigger value="dns-01">
                  <Network aria-hidden="true" />
                  DNS-01
                </TabsTrigger>
              </TabsList>
            </Tabs>
          </div>
          <FormField
            v-if="form.challenge === 'dns-01'"
            id="automatic-provider"
            :label="t('certificates.dns.provider')"
            :hint="providers.length === 0 ? t('certificates.dns.noneYet') : undefined"
          >
            <Select v-model="form.provider" :disabled="providers.length === 0">
              <SelectTrigger id="automatic-provider" class="w-full">
                <SelectValue :placeholder="t('certificates.dns.pick')" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem v-for="provider in providers" :key="provider.id" :value="provider.id">
                  <span class="font-mono text-xs">{{ provider.id }}</span>
                  <span class="text-muted-foreground truncate text-xs">{{
                    ` · ${provider.rfc2136.zones.join(', ')}`
                  }}</span>
                </SelectItem>
              </SelectContent>
            </Select>
          </FormField>
          <Alert>
            <Info aria-hidden="true" />
            <AlertDescription>{{
              form.challenge === 'http-01'
                ? t('certificates.acme.http01Detail')
                : t('certificates.acme.dns01Detail')
            }}</AlertDescription>
          </Alert>
        </div>
        <SheetFooter>
          <Button type="submit" :disabled="create.isPending.value || !ready">
            <CalendarSync data-icon="inline-start" aria-hidden="true" />
            {{ t('certificates.acme.request') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
