<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { Network } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { DnsProviderView, Rfc2136Settings, TsigAlgorithm } from '@/api/generated'
import {
  createDnsProviderMutation,
  updateDnsProviderMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import { Button } from '@/components/ui/button'
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
import { changeHeaders, notifyFailure, plainHeaders } from '@/lib/configuration'
import { ACCOUNT_ID, parseNames } from './presentation'

const ALGORITHMS: readonly TsigAlgorithm[] = ['hmac-sha256', 'hmac-sha512']

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{
  /** Edited when given, added otherwise. */
  provider?: DnsProviderView
  taken: readonly string[]
}>()
const emit = defineEmits<{ saved: [] }>()

const { t } = useI18n()
const create = useMutation(createDnsProviderMutation())
const update = useMutation(updateDnsProviderMutation())
const busy = computed(() => create.isPending.value || update.isPending.value)

function blank() {
  const provider = props.provider
  return {
    id: provider?.id ?? '',
    server: provider?.rfc2136.server ?? '',
    zones: provider?.rfc2136.zones.join(', ') ?? '',
    keyName: provider?.rfc2136.key_name ?? '',
    algorithm: (provider?.rfc2136.algorithm ?? 'hmac-sha256') as TsigAlgorithm,
    secret: '',
    ttl: provider?.rfc2136.ttl ?? undefined,
    propagation: provider?.propagation_seconds ?? 30,
  }
}
const form = reactive(blank())
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, blank())
  }
})

const editing = computed(() => props.provider !== undefined)
const zones = computed(() => parseNames(form.zones))
const idError = computed(() => {
  if (editing.value || form.id === '') {
    return null
  }
  if (!ACCOUNT_ID.test(form.id)) {
    return t('certificates.acme.accountIdHint')
  }
  return props.taken.includes(form.id) ? t('certificates.dns.idTaken') : null
})
const serverValid = computed(() => /^\S+:\d{1,5}$/.test(form.server.trim()))
const propagationValid = computed(
  () => Number.isInteger(form.propagation) && form.propagation >= 0 && form.propagation <= 3600,
)
const ready = computed(
  () =>
    (editing.value || (form.id !== '' && idError.value === null)) &&
    serverValid.value &&
    zones.value.length > 0 &&
    form.keyName.trim() !== '' &&
    (editing.value || form.secret.trim() !== '') &&
    propagationValid.value,
)

function settings(): Rfc2136Settings {
  const settings: Rfc2136Settings = {
    server: form.server.trim(),
    zones: zones.value,
    key_name: form.keyName.trim(),
    algorithm: form.algorithm,
  }
  if (typeof form.ttl === 'number' && form.ttl > 0) {
    settings.ttl = form.ttl
  }
  return settings
}

function saved(id: string) {
  toast.success(t(editing.value ? 'certificates.dns.updated' : 'certificates.dns.added', { id }))
  open.value = false
  emit('saved')
}

function submit() {
  const onError = (error: unknown) => notifyFailure(error, t('common.changeFailed'))
  const provider = props.provider
  if (provider) {
    update.mutate(
      {
        path: { id: provider.id },
        body: {
          rfc2136: settings(),
          propagation_seconds: form.propagation,
          ...(form.secret.trim() === '' ? {} : { secret: form.secret.trim() }),
        },
        headers: changeHeaders(provider.etag),
      },
      { onSuccess: (changed) => saved(changed.id), onError },
    )
  } else {
    create.mutate(
      {
        body: {
          id: form.id,
          kind: 'rfc2136',
          rfc2136: settings(),
          secret: form.secret.trim(),
          propagation_seconds: form.propagation,
        },
        headers: plainHeaders(),
      },
      { onSuccess: (created) => saved(created.id), onError },
    )
  }
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle class="flex items-center gap-2">
            <Network class="size-5" aria-hidden="true" />
            {{
              editing
                ? t('certificates.dns.edit', { id: provider?.id ?? '' })
                : t('certificates.dns.add')
            }}
          </SheetTitle>
          <SheetDescription>{{ t('certificates.dns.addDetail') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-4 px-4">
          <FormField
            v-if="!editing"
            id="dns-id"
            :label="t('certificates.dns.id')"
            :hint="idError ?? t('certificates.acme.accountIdHint')"
          >
            <Input
              id="dns-id"
              v-model="form.id"
              required
              :aria-invalid="idError !== null"
              class="font-mono text-xs"
              autocomplete="off"
            />
          </FormField>
          <FormField
            id="dns-server"
            :label="t('certificates.dns.server')"
            :hint="t('certificates.dns.serverHint')"
          >
            <Input
              id="dns-server"
              v-model="form.server"
              required
              class="font-mono text-xs"
              placeholder="ns1.example.com:53"
              autocomplete="off"
            />
          </FormField>
          <FormField
            id="dns-zones"
            :label="t('certificates.dns.zones')"
            :hint="t('certificates.dns.zonesHint')"
          >
            <Input
              id="dns-zones"
              v-model="form.zones"
              required
              class="font-mono text-xs"
              placeholder="example.com"
              autocomplete="off"
            />
          </FormField>
          <div class="grid gap-4 sm:grid-cols-2">
            <FormField id="dns-key-name" :label="t('certificates.dns.keyName')">
              <Input
                id="dns-key-name"
                v-model="form.keyName"
                required
                class="font-mono text-xs"
                autocomplete="off"
              />
            </FormField>
            <FormField id="dns-algorithm" :label="t('certificates.dns.algorithm')">
              <Select v-model="form.algorithm">
                <SelectTrigger id="dns-algorithm" class="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="algorithm in ALGORITHMS" :key="algorithm" :value="algorithm">
                    {{ algorithm }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </FormField>
          </div>
          <FormField
            id="dns-secret"
            :label="t('certificates.dns.secret')"
            :hint="editing ? t('certificates.dns.secretKeep') : t('certificates.dns.secretHint')"
          >
            <Input
              id="dns-secret"
              v-model="form.secret"
              type="password"
              :required="!editing"
              class="font-mono text-xs"
              autocomplete="off"
            />
          </FormField>
          <div class="grid gap-4 sm:grid-cols-2">
            <FormField
              id="dns-propagation"
              :label="t('certificates.dns.propagation')"
              :hint="t('certificates.dns.propagationHint')"
            >
              <Input
                id="dns-propagation"
                v-model.number="form.propagation"
                type="number"
                min="0"
                max="3600"
                required
              />
            </FormField>
            <FormField
              id="dns-ttl"
              :label="t('certificates.dns.ttl')"
              :hint="t('certificates.dns.ttlHint')"
            >
              <Input
                id="dns-ttl"
                v-model.number="form.ttl"
                type="number"
                min="1"
                placeholder="60"
              />
            </FormField>
          </div>
        </div>
        <SheetFooter>
          <Button type="submit" :disabled="busy || !ready">
            <Network data-icon="inline-start" aria-hidden="true" />
            {{ editing ? t('common.save') : t('certificates.dns.add') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
