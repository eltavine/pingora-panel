<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Save } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { SiteKind, SiteView } from '@/api/generated'
import {
  createSiteMutation,
  listListenersOptions,
  listTlsProfilesOptions,
  replaceSiteMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ChoiceCards from '@/components/ChoiceCards.vue'
import FormField from '@/components/FormField.vue'
import SwitchField from '@/components/SwitchField.vue'
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
import { Textarea } from '@/components/ui/textarea'
import {
  changeHeaders,
  notifyFailure,
  plainHeaders,
  useRefreshConfiguration,
} from '@/lib/configuration'
import SecurityPolicySelect from '@/features/security/SecurityPolicySelect.vue'
import AccessLogFields from './AccessLogFields.vue'
import ActionFields from './ActionFields.vue'
import {
  invalidFieldLines,
  PRELOAD_DAYS,
  SITE_KINDS,
  WWW_REDIRECTS,
  siteForm,
  siteInput,
  siteKindAction,
  type SiteForm,
} from './forms'
import { kindIcons } from './presentation'

const NO_PROFILE = '-'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ site?: SiteView }>()
const emit = defineEmits<{ saved: [site: SiteView] }>()

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const listeners = useQuery(listListenersOptions())
const profiles = useQuery(listTlsProfilesOptions())
const create = useMutation(createSiteMutation())
const replace = useMutation(replaceSiteMutation())
const busy = computed(() => create.isPending.value || replace.isPending.value)

const form = reactive<SiteForm>(siteForm())
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, siteForm(props.site))
  }
})

const kinds = computed(() =>
  SITE_KINDS.map((value) => ({ value, label: t(`sites.kind.${value}`), icon: kindIcons[value] })),
)
const kind = computed<SiteKind>({
  get: () =>
    SITE_KINDS.find((value) => siteKindAction[value] === form.action.type) ?? 'reverse_proxy',
  set: (value) => (form.action.type = siteKindAction[value]),
})
const preloadReady = computed(
  () => form.hsts.includeSubdomains && Number(form.hsts.maxAgeDays) >= PRELOAD_DAYS,
)
const profile = computed({
  get: () => form.tlsProfileId || NO_PROFILE,
  set: (value: string) => (form.tlsProfileId = value === NO_PROFILE ? '' : value),
})

function toggleListener(id: string, checked: boolean | 'indeterminate') {
  form.listenerIds =
    checked === true ? [...form.listenerIds, id] : form.listenerIds.filter((value) => value !== id)
}

function saved(site: SiteView) {
  toast.success(props.site ? t('common.saved') : t('sites.created', { name: site.name }))
  open.value = false
  emit('saved', site)
  void refresh()
}

function submit() {
  const body = siteInput(form, props.site)
  const onError = (error: unknown) => notifyFailure(error, t('common.changeFailed'))
  if (props.site) {
    replace.mutate(
      { path: { id: props.site.id }, body, headers: changeHeaders(props.site.etag) },
      { onSuccess: saved, onError },
    )
  } else {
    create.mutate({ body, headers: plainHeaders() }, { onSuccess: saved, onError })
  }
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-xl">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>{{ site ? t('sites.edit') : t('sites.new') }}</SheetTitle>
          <SheetDescription>{{ t('sites.description') }}</SheetDescription>
        </SheetHeader>

        <div class="flex flex-col gap-4 px-4">
          <FormField id="site-name" :label="t('sites.form.name')">
            <Input id="site-name" v-model="form.name" required maxlength="128" autocomplete="off" />
          </FormField>

          <div class="flex flex-col gap-1.5">
            <Label>{{ t('sites.form.kind') }}</Label>
            <ChoiceCards v-model="kind" :label="t('sites.form.kind')" :choices="kinds" />
          </div>

          <ActionFields v-model="form.action" id-prefix="site-action" />

          <FormField
            v-if="!site"
            id="site-domains"
            :label="t('sites.form.domains')"
            :hint="t('sites.form.domainsHint')"
          >
            <Textarea
              id="site-domains"
              v-model="form.domains"
              rows="3"
              class="font-mono text-xs"
              placeholder="example.com&#10;www.example.com"
            />
          </FormField>

          <SwitchField
            id="site-https-redirect"
            v-model="form.httpsRedirect"
            :label="t('sites.form.httpsRedirect')"
          />
          <SwitchField
            id="site-hsts"
            v-model="form.hsts.enabled"
            :label="t('sites.form.hsts')"
            :hint="t('sites.form.hstsHint')"
          />
          <div v-if="form.hsts.enabled" class="flex flex-col gap-3 rounded-md border p-3">
            <FormField id="site-hsts-age" :label="t('sites.form.hstsMaxAge')">
              <Input
                id="site-hsts-age"
                v-model="form.hsts.maxAgeDays"
                type="number"
                min="0"
                class="w-32"
              />
            </FormField>
            <label class="flex items-center gap-2 text-sm">
              <Checkbox v-model="form.hsts.includeSubdomains" />
              {{ t('sites.form.hstsSubdomains') }}
            </label>
            <label class="flex items-center gap-2 text-sm">
              <Checkbox v-model="form.hsts.preload" />
              {{ t('sites.form.hstsPreload') }}
            </label>
            <p
              v-if="form.hsts.preload"
              class="text-xs"
              :class="preloadReady ? 'text-muted-foreground' : 'text-destructive'"
            >
              {{ t('sites.form.hstsPreloadHint') }}
            </p>
          </div>

          <FormField id="site-www" :label="t('sites.form.wwwRedirect')">
            <Select v-model="form.wwwRedirect">
              <SelectTrigger id="site-www" class="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem v-for="mode in WWW_REDIRECTS" :key="mode" :value="mode">
                  {{
                    mode === 'none'
                      ? t('sites.form.wwwNone')
                      : mode === 'add_www'
                        ? t('sites.form.wwwAdd')
                        : t('sites.form.wwwRemove')
                  }}
                </SelectItem>
              </SelectContent>
            </Select>
          </FormField>

          <fieldset class="flex flex-col gap-2">
            <legend class="mb-1.5 text-sm font-medium">{{ t('sites.form.listeners') }}</legend>
            <label
              v-for="listener in listeners.data.value ?? []"
              :key="listener.id"
              class="flex items-center gap-2 text-sm"
            >
              <Checkbox
                :model-value="form.listenerIds.includes(listener.id)"
                @update:model-value="toggleListener(listener.id, $event)"
              />
              <span class="font-mono text-xs">{{ listener.id }}</span>
              <span class="text-muted-foreground font-mono text-xs">{{ listener.address }}</span>
            </label>
            <p class="text-muted-foreground text-xs">{{ t('sites.form.listenersHint') }}</p>
          </fieldset>

          <FormField id="site-tls" :label="t('sites.form.tlsProfile')">
            <Select v-model="profile">
              <SelectTrigger id="site-tls" class="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem :value="NO_PROFILE">{{ t('sites.form.noTlsProfile') }}</SelectItem>
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

          <SecurityPolicySelect
            id="site-security-policy"
            v-model="form.securityPolicyId"
            :hint="t('security.select.siteHint')"
          />

          <AccessLogFields v-model="form.accessLog" id-prefix="site-access-log" scope="site" />

          <div class="grid gap-4 sm:grid-cols-2">
            <FormField id="site-group" :label="t('sites.form.group')">
              <Input id="site-group" v-model="form.group" autocomplete="off" />
            </FormField>
            <FormField
              id="site-tags"
              :label="t('sites.form.tags')"
              :hint="t('sites.form.tagsHint')"
            >
              <Input id="site-tags" v-model="form.tags" autocomplete="off" />
            </FormField>
          </div>

          <FormField id="site-note" :label="t('sites.form.note')">
            <Textarea id="site-note" v-model="form.note" rows="2" />
          </FormField>
        </div>

        <SheetFooter>
          <Button
            type="submit"
            :disabled="busy || invalidFieldLines(form.accessLog.fields).length > 0"
          >
            <Save data-icon="inline-start" aria-hidden="true" />
            {{ site ? t('common.save') : t('common.create') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
