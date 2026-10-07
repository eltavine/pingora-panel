<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Construction, Save } from '@lucide/vue'
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
import HttpPolicySelect from '@/components/HttpPolicySelect.vue'
import SecurityPolicySelect from '@/components/SecurityPolicySelect.vue'
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
import ErrorPagesEditor from './ErrorPagesEditor.vue'
import {
  FAVICON_CHOICES,
  faviconProblem,
  maintenanceProblem,
  pagesProblem,
  ROBOTS_CHOICES,
  robotsProblem,
} from './pages'
import { faviconIcons, kindIcons, robotsIcons } from './presentation'
import { rewriteProblem } from './rewrites'
import RewritesEditor from './RewritesEditor.vue'

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

const maintenanceIssue = computed(() => {
  const found = maintenanceProblem(form.maintenance)
  return found ? t(found.key, found.values ?? {}) : undefined
})
const unfinished = computed(
  () =>
    invalidFieldLines(form.accessLog.fields).length > 0 ||
    form.rewrites.some((rule) => rewriteProblem(rule)) ||
    pagesProblem(form.errorPages) ||
    Boolean(maintenanceIssue.value) ||
    Boolean(robotsProblem(form.robots)) ||
    Boolean(faviconProblem(form.favicon)),
)

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

          <HttpPolicySelect
            id="site-http-policy"
            v-model="form.httpPolicyId"
            :hint="t('httpPolicies.select.siteHint')"
          />

          <fieldset class="flex flex-col gap-2">
            <legend class="text-sm font-medium">{{ t('routes.rewrites.title') }}</legend>
            <p class="text-muted-foreground text-xs">{{ t('routes.rewrites.siteDescription') }}</p>
            <RewritesEditor v-model="form.rewrites" id-prefix="site-rewrite" />
          </fieldset>

          <fieldset class="flex flex-col gap-2">
            <legend class="text-sm font-medium">{{ t('sites.errorPages.title') }}</legend>
            <p class="text-muted-foreground text-xs">{{ t('sites.errorPages.siteDescription') }}</p>
            <ErrorPagesEditor v-model="form.errorPages" id-prefix="site-page" />
          </fieldset>

          <fieldset class="flex flex-col gap-2">
            <legend class="flex items-center gap-2 text-sm font-medium">
              <Construction class="text-muted-foreground size-4" aria-hidden="true" />
              {{ t('sites.maintenance.title') }}
            </legend>
            <SwitchField
              id="site-maintenance"
              v-model="form.maintenance.enabled"
              :label="t('sites.maintenance.enabled')"
              :hint="t('sites.maintenance.description')"
            />
            <div v-if="form.maintenance.enabled" class="flex flex-col gap-3 rounded-md border p-3">
              <FormField
                id="site-maintenance-allow"
                :label="t('sites.maintenance.allow')"
                :hint="t('sites.maintenance.allowHint')"
              >
                <Textarea
                  id="site-maintenance-allow"
                  v-model="form.maintenance.allow"
                  rows="3"
                  class="font-mono text-xs"
                  placeholder="10.0.0.0/8&#10;2001:db8::1"
                />
              </FormField>
              <div class="grid gap-4 sm:grid-cols-2">
                <FormField id="site-maintenance-status" :label="t('sites.maintenance.status')">
                  <Input
                    id="site-maintenance-status"
                    v-model="form.maintenance.status"
                    type="number"
                    min="200"
                    max="599"
                  />
                </FormField>
                <FormField id="site-maintenance-retry" :label="t('sites.form.retryAfter')">
                  <Input
                    id="site-maintenance-retry"
                    v-model="form.maintenance.retryAfter"
                    type="number"
                    min="0"
                  />
                </FormField>
              </div>
              <FormField
                id="site-maintenance-body"
                :label="t('sites.maintenance.body')"
                :hint="t('sites.maintenance.bodyHint')"
              >
                <Textarea
                  id="site-maintenance-body"
                  v-model="form.maintenance.body"
                  rows="3"
                  class="font-mono text-xs"
                />
              </FormField>
              <FormField id="site-maintenance-type" :label="t('sites.form.contentType')">
                <Input
                  id="site-maintenance-type"
                  v-model="form.maintenance.contentType"
                  class="font-mono text-xs"
                  autocomplete="off"
                  placeholder="text/html; charset=utf-8"
                />
              </FormField>
              <p v-if="maintenanceIssue" class="text-destructive text-xs" role="alert">
                {{ maintenanceIssue }}
              </p>
            </div>
          </fieldset>

          <div class="grid gap-4 sm:grid-cols-2">
            <div class="flex flex-col gap-2">
              <FormField
                id="site-robots"
                :label="t('sites.robots.title')"
                :hint="t('sites.robots.description')"
              >
                <Select v-model="form.robots.choice">
                  <SelectTrigger id="site-robots" class="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem v-for="choice in ROBOTS_CHOICES" :key="choice" :value="choice">
                      <component :is="robotsIcons[choice]" aria-hidden="true" />
                      {{ t(`sites.robots.choices.${choice}`) }}
                    </SelectItem>
                  </SelectContent>
                </Select>
              </FormField>
              <Textarea
                v-if="form.robots.choice === 'custom'"
                id="site-robots-body"
                v-model="form.robots.body"
                rows="4"
                class="font-mono text-xs"
                :aria-label="t('sites.robots.body')"
              />
              <p v-if="robotsProblem(form.robots)" class="text-destructive text-xs" role="alert">
                {{ t(robotsProblem(form.robots) ?? '') }}
              </p>
            </div>
            <div class="flex flex-col gap-2">
              <FormField
                id="site-favicon"
                :label="t('sites.favicon.title')"
                :hint="t('sites.favicon.description')"
              >
                <Select v-model="form.favicon.choice">
                  <SelectTrigger id="site-favicon" class="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem v-for="choice in FAVICON_CHOICES" :key="choice" :value="choice">
                      <component :is="faviconIcons[choice]" aria-hidden="true" />
                      {{ t(`sites.favicon.choices.${choice}`) }}
                    </SelectItem>
                  </SelectContent>
                </Select>
              </FormField>
              <Input
                v-if="form.favicon.choice === 'file'"
                id="site-favicon-path"
                v-model="form.favicon.path"
                class="font-mono text-xs"
                autocomplete="off"
                placeholder="shop/favicon.ico"
                :aria-label="t('sites.favicon.path')"
              />
              <Input
                v-else-if="form.favicon.choice === 'redirect'"
                id="site-favicon-location"
                v-model="form.favicon.location"
                class="font-mono text-xs"
                autocomplete="off"
                placeholder="https://cdn.example.com/favicon.ico"
                :aria-label="t('sites.favicon.location')"
              />
              <p v-if="faviconProblem(form.favicon)" class="text-destructive text-xs" role="alert">
                {{ t(faviconProblem(form.favicon) ?? '') }}
              </p>
            </div>
          </div>

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
          <Button type="submit" :disabled="busy || unfinished">
            <Save data-icon="inline-start" aria-hidden="true" />
            {{ site ? t('common.save') : t('common.create') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
