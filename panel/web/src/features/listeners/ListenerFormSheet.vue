<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Save } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { ListenerView } from '@/api/generated'
import {
  listSitesOptions,
  listTlsProfilesOptions,
  putListenerMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import SwitchField from '@/components/SwitchField.vue'
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
import {
  changeHeaders,
  notifyFailure,
  plainHeaders,
  useRefreshConfiguration,
} from '@/lib/configuration'
import { RESOURCE_ID, listenerBody, listenerForm, type ListenerForm } from './forms'

const NONE = '-'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ listener?: ListenerView; taken: readonly string[] }>()

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const profiles = useQuery(listTlsProfilesOptions())
const sites = useQuery(listSitesOptions({ query: { limit: 500 } }))
const put = useMutation(putListenerMutation())

const form = reactive<ListenerForm>(listenerForm())
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, listenerForm(props.listener))
  }
})

const idError = computed(() => {
  if (props.listener || form.id === '') {
    return null
  }
  if (!RESOURCE_ID.test(form.id)) {
    return t('listeners.idHint')
  }
  return props.taken.includes(form.id) ? t('listeners.idTaken') : null
})
const profile = computed({
  get: () => form.tlsProfileId || NONE,
  set: (value: string) => (form.tlsProfileId = value === NONE ? '' : value),
})
const defaultSite = computed({
  get: () => form.defaultSiteId || NONE,
  set: (value: string) => (form.defaultSiteId = value === NONE ? '' : value),
})

function submit() {
  const body = listenerBody(form)
  put.mutate(
    {
      path: { id: body.id },
      body,
      headers: props.listener ? changeHeaders(props.listener.etag) : plainHeaders(),
    },
    {
      onSuccess: (saved) => {
        toast.success(t('listeners.saved', { id: saved.id }))
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
          <SheetTitle>{{ listener ? t('listeners.edit') : t('listeners.new') }}</SheetTitle>
          <SheetDescription>{{ t('listeners.description') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-4 px-4">
          <FormField
            id="listener-id"
            :label="t('listeners.id')"
            :hint="idError ?? t('listeners.idHint')"
          >
            <Input
              id="listener-id"
              v-model="form.id"
              required
              :disabled="Boolean(listener)"
              :aria-invalid="idError !== null"
              class="font-mono text-xs"
              autocomplete="off"
            />
          </FormField>
          <FormField
            id="listener-address"
            :label="t('listeners.address')"
            :hint="t('listeners.addressHint')"
          >
            <Input
              id="listener-address"
              v-model="form.address"
              required
              class="font-mono text-xs"
              autocomplete="off"
            />
          </FormField>
          <fieldset class="flex flex-col gap-2">
            <legend class="mb-1.5 text-sm font-medium">{{ t('listeners.protocols') }}</legend>
            <SwitchField id="listener-http1" v-model="form.http1" :label="t('listeners.http1')" />
            <SwitchField id="listener-http2" v-model="form.http2" :label="t('listeners.http2')" />
            <SwitchField
              id="listener-http3"
              v-model="form.http3"
              :label="t('listeners.http3')"
              :hint="t('listeners.http3Reserved')"
              :disabled="!form.http3"
            />
          </fieldset>
          <FormField id="listener-tls" :label="t('listeners.tlsProfile')">
            <Select v-model="profile">
              <SelectTrigger id="listener-tls" class="w-full"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem :value="NONE">{{ t('listeners.plain') }}</SelectItem>
                <SelectItem
                  v-for="item in profiles.data.value ?? []"
                  :key="item.id"
                  :value="item.id"
                >
                  {{ item.id }}
                </SelectItem>
              </SelectContent>
            </Select>
          </FormField>
          <FormField
            id="listener-default"
            :label="t('listeners.defaultSite')"
            :hint="t('listeners.defaultSiteHint')"
          >
            <Select v-model="defaultSite">
              <SelectTrigger id="listener-default" class="w-full"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem :value="NONE">{{ t('state.none') }}</SelectItem>
                <SelectItem
                  v-for="site in sites.data.value?.items ?? []"
                  :key="site.id"
                  :value="site.id"
                >
                  {{ site.name }}
                </SelectItem>
              </SelectContent>
            </Select>
          </FormField>
          <SwitchField
            id="listener-reuse"
            v-model="form.reusePort"
            :label="t('listeners.reusePort')"
          />
          <SwitchField
            id="listener-v6only"
            v-model="form.ipv6Only"
            :label="t('listeners.ipv6Only')"
          />
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
