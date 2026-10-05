<script setup lang="ts">
import { computed, reactive, watch, type Component } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import {
  Gauge,
  KeyRound,
  Link2,
  ListFilter,
  MessageSquareWarning,
  Network,
  Plus,
  Ruler,
  Save,
  Trash2,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { RateLimitKey, SecurityPolicyView } from '@/api/generated'
import { putSecurityPolicyMutation } from '@/api/generated/@tanstack/vue-query.gen'
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
import { Textarea } from '@/components/ui/textarea'
import { RESOURCE_ID } from '@/lib/forms'
import {
  changeHeaders,
  notifyFailure,
  plainHeaders,
  useRefreshConfiguration,
} from '@/lib/configuration'
import { parseSize } from '@/lib/forms'
import {
  KEY_KINDS,
  METHODS,
  PERIODS,
  policyBody,
  policyForm,
  rateLimitForm,
  type PolicyForm,
  type RateLimitForm,
} from './forms'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ policy?: SecurityPolicyView; taken: readonly string[] }>()

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const put = useMutation(putSecurityPolicyMutation())

const form = reactive<PolicyForm>(policyForm())
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, policyForm(props.policy))
  }
})

const idError = computed(() => {
  if (props.policy || form.id === '') {
    return null
  }
  if (!RESOURCE_ID.test(form.id)) {
    return t('security.idHint')
  }
  return props.taken.includes(form.id) ? t('security.idTaken') : null
})
const headerSizeInvalid = computed(() => Number.isNaN(parseSize(form.maxHeaderSize)))
const bodySizeInvalid = computed(() => Number.isNaN(parseSize(form.maxBodySize)))
const invalid = computed(
  () => idError.value !== null || headerSizeInvalid.value || bodySizeInvalid.value,
)

function toggleMethod(method: string, checked: boolean | 'indeterminate') {
  form.methods =
    checked === true
      ? METHODS.filter((value) => value === method || form.methods.includes(value))
      : form.methods.filter((value) => value !== method)
}

/** The named periods, and the limit's own when it is none of them. */
function periods(limit: RateLimitForm) {
  return PERIODS.some((period) => period.seconds === limit.perSeconds)
    ? PERIODS
    : [...PERIODS, { seconds: limit.perSeconds, name: 'custom' }]
}

function periodLabel(period: { seconds: number; name: string }) {
  return period.name === 'custom'
    ? t('security.periods.custom', { count: period.seconds })
    : t(`security.periods.${period.name}`)
}

function setKey(limit: RateLimitForm, value: unknown) {
  limit.key = value as RateLimitKey['kind']
}

function submit() {
  const body = policyBody(form)
  put.mutate(
    {
      path: { id: body.id },
      body,
      headers: props.policy ? changeHeaders(props.policy.etag) : plainHeaders(),
    },
    {
      onSuccess: (saved) => {
        toast.success(t('security.saved', { id: saved.id }))
        open.value = false
        void refresh()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

const sections: Record<string, Component> = {
  clients: Network,
  requests: ListFilter,
  referers: Link2,
  password: KeyRound,
  limits: Ruler,
  rates: Gauge,
  response: MessageSquareWarning,
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-xl">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>{{ policy ? t('security.edit') : t('security.new') }}</SheetTitle>
          <SheetDescription>{{ t('security.description') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-6 px-4">
          <FormField
            id="policy-id"
            :label="t('security.id')"
            :hint="idError ?? t('security.idHint')"
          >
            <Input
              id="policy-id"
              v-model="form.id"
              required
              :disabled="Boolean(policy)"
              :aria-invalid="idError !== null"
              class="font-mono text-xs"
              autocomplete="off"
            />
          </FormField>

          <fieldset class="flex flex-col gap-4">
            <legend class="mb-3 flex items-center gap-2 text-sm font-medium">
              <component :is="sections.clients" class="size-4" aria-hidden="true" />
              {{ t('security.sections.clients') }}
            </legend>
            <FormField
              id="policy-allow"
              :label="t('security.allowedCidrs')"
              :hint="t('security.allowedCidrsHint')"
            >
              <Textarea
                id="policy-allow"
                v-model="form.allowedCidrs"
                rows="2"
                class="font-mono text-xs"
                placeholder="10.0.0.0/8&#10;2001:db8::/32"
              />
            </FormField>
            <FormField
              id="policy-deny"
              :label="t('security.deniedCidrs')"
              :hint="t('security.deniedCidrsHint')"
            >
              <Textarea
                id="policy-deny"
                v-model="form.deniedCidrs"
                rows="2"
                class="font-mono text-xs"
              />
            </FormField>
          </fieldset>

          <fieldset class="flex flex-col gap-4">
            <legend class="mb-3 flex items-center gap-2 text-sm font-medium">
              <component :is="sections.requests" class="size-4" aria-hidden="true" />
              {{ t('security.sections.requests') }}
            </legend>
            <div class="flex flex-col gap-2">
              <span class="text-sm font-medium">{{ t('security.methods') }}</span>
              <div class="flex flex-wrap gap-x-4 gap-y-2">
                <label
                  v-for="method in METHODS"
                  :key="method"
                  class="flex items-center gap-2 font-mono text-xs"
                >
                  <Checkbox
                    :model-value="form.methods.includes(method)"
                    @update:model-value="toggleMethod(method, $event)"
                  />
                  {{ method }}
                </label>
              </div>
              <p class="text-muted-foreground text-xs">{{ t('security.methodsHint') }}</p>
            </div>
            <FormField
              id="policy-paths"
              :label="t('security.deniedPaths')"
              :hint="t('security.deniedPathsHint')"
            >
              <Textarea
                id="policy-paths"
                v-model="form.deniedPaths"
                rows="2"
                class="font-mono text-xs"
                placeholder="/.git&#10;/admin/internal"
              />
            </FormField>
            <FormField
              id="policy-agents"
              :label="t('security.deniedUserAgents')"
              :hint="t('security.deniedUserAgentsHint')"
            >
              <Textarea
                id="policy-agents"
                v-model="form.deniedUserAgents"
                rows="2"
                class="font-mono text-xs"
                placeholder="^curl/&#10;sqlmap"
              />
            </FormField>
          </fieldset>

          <fieldset class="flex flex-col gap-3">
            <legend class="mb-3 flex items-center gap-2 text-sm font-medium">
              <component :is="sections.referers" class="size-4" aria-hidden="true" />
              {{ t('security.sections.referers') }}
            </legend>
            <SwitchField
              id="policy-referer"
              v-model="form.referer"
              :label="t('security.referer')"
              :hint="t('security.refererHint')"
            />
            <template v-if="form.referer">
              <FormField
                id="policy-referer-hosts"
                :label="t('security.refererHosts')"
                :hint="t('security.refererHostsHint')"
              >
                <Textarea
                  id="policy-referer-hosts"
                  v-model="form.refererHosts"
                  rows="2"
                  class="font-mono text-xs"
                  placeholder="example.com&#10;*.example.com"
                />
              </FormField>
              <label class="flex items-center gap-2 text-sm">
                <Checkbox v-model="form.allowEmptyReferer" />
                {{ t('security.allowEmptyReferer') }}
              </label>
            </template>
          </fieldset>

          <fieldset class="flex flex-col gap-3">
            <legend class="mb-3 flex items-center gap-2 text-sm font-medium">
              <component :is="sections.password" class="size-4" aria-hidden="true" />
              {{ t('security.sections.password') }}
            </legend>
            <SwitchField
              id="policy-basic-auth"
              v-model="form.basicAuth"
              :label="t('security.basicAuth')"
              :hint="t('security.basicAuthHint')"
            />
            <div v-if="form.basicAuth" class="grid gap-4 sm:grid-cols-2">
              <FormField
                id="policy-users"
                :label="t('security.usersFile')"
                :hint="t('security.usersFileHint')"
              >
                <Input
                  id="policy-users"
                  v-model="form.usersFile"
                  required
                  class="font-mono text-xs"
                  placeholder="staff.htpasswd"
                  autocomplete="off"
                />
              </FormField>
              <FormField id="policy-realm" :label="t('security.realm')">
                <Input id="policy-realm" v-model="form.realm" required autocomplete="off" />
              </FormField>
            </div>
          </fieldset>

          <fieldset class="flex flex-col gap-4">
            <legend class="mb-3 flex items-center gap-2 text-sm font-medium">
              <component :is="sections.limits" class="size-4" aria-hidden="true" />
              {{ t('security.sections.limits') }}
            </legend>
            <div class="grid gap-4 sm:grid-cols-2">
              <FormField
                id="policy-header-size"
                :label="t('security.maxHeaderSize')"
                :hint="headerSizeInvalid ? t('security.invalidSize') : undefined"
              >
                <Input
                  id="policy-header-size"
                  v-model="form.maxHeaderSize"
                  :aria-invalid="headerSizeInvalid"
                  class="font-mono text-xs"
                  placeholder="16k"
                  autocomplete="off"
                />
              </FormField>
              <FormField
                id="policy-body-size"
                :label="t('security.maxBodySize')"
                :hint="bodySizeInvalid ? t('security.invalidSize') : undefined"
              >
                <Input
                  id="policy-body-size"
                  v-model="form.maxBodySize"
                  :aria-invalid="bodySizeInvalid"
                  class="font-mono text-xs"
                  placeholder="10m"
                  autocomplete="off"
                />
              </FormField>
            </div>
            <FormField id="policy-body-timeout" :label="t('security.bodyTimeout')">
              <Input
                id="policy-body-timeout"
                v-model="form.bodyTimeout"
                type="number"
                min="1"
                max="3600"
                class="w-32"
              />
            </FormField>
            <p class="text-muted-foreground -mt-2 text-xs">{{ t('security.limitsHint') }}</p>
          </fieldset>

          <fieldset class="flex flex-col gap-3">
            <legend class="mb-3 flex w-full items-center gap-2 text-sm font-medium">
              <component :is="sections.rates" class="size-4" aria-hidden="true" />
              {{ t('security.sections.rates') }}
            </legend>
            <div
              v-for="(limit, index) in form.rateLimits"
              :key="index"
              class="flex flex-col gap-3 rounded-md border p-3"
            >
              <div class="grid grid-cols-[1fr_1fr_1fr_auto] items-end gap-2">
                <FormField :id="`policy-rate-${index}`" :label="t('security.requests')">
                  <Input
                    :id="`policy-rate-${index}`"
                    v-model="limit.requests"
                    type="number"
                    min="1"
                    required
                  />
                </FormField>
                <FormField :id="`policy-period-${index}`" :label="t('security.per')">
                  <Select
                    :model-value="String(limit.perSeconds)"
                    @update:model-value="limit.perSeconds = Number($event)"
                  >
                    <SelectTrigger :id="`policy-period-${index}`" class="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem
                        v-for="period in periods(limit)"
                        :key="period.seconds"
                        :value="String(period.seconds)"
                      >
                        {{ periodLabel(period) }}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </FormField>
                <FormField :id="`policy-burst-${index}`" :label="t('security.burst')">
                  <Input
                    :id="`policy-burst-${index}`"
                    v-model="limit.burst"
                    type="number"
                    min="0"
                  />
                </FormField>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  :aria-label="t('security.removeRateLimit')"
                  @click="form.rateLimits.splice(index, 1)"
                >
                  <Trash2 aria-hidden="true" />
                </Button>
              </div>
              <div class="grid gap-2 sm:grid-cols-2">
                <FormField :id="`policy-key-${index}`" :label="t('security.key')">
                  <Select :model-value="limit.key" @update:model-value="setKey(limit, $event)">
                    <SelectTrigger :id="`policy-key-${index}`" class="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem v-for="kind in KEY_KINDS" :key="kind" :value="kind">
                        {{ t(`security.keys.${kind}`) }}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </FormField>
                <FormField
                  v-if="limit.key === 'header'"
                  :id="`policy-header-${index}`"
                  :label="t('security.headerName')"
                >
                  <Input
                    :id="`policy-header-${index}`"
                    v-model="limit.header"
                    required
                    class="font-mono text-xs"
                    placeholder="X-Api-Key"
                    autocomplete="off"
                  />
                </FormField>
              </div>
            </div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              class="self-start"
              @click="form.rateLimits.push(rateLimitForm())"
            >
              <Plus data-icon="inline-start" aria-hidden="true" />
              {{ t('security.addRateLimit') }}
            </Button>
            <FormField
              id="policy-concurrent"
              :label="t('security.maxConcurrent')"
              :hint="t('security.maxConcurrentHint')"
            >
              <Input
                id="policy-concurrent"
                v-model="form.maxConcurrent"
                type="number"
                min="1"
                class="w-32"
              />
            </FormField>
          </fieldset>

          <fieldset class="flex flex-col gap-3">
            <legend class="mb-3 flex items-center gap-2 text-sm font-medium">
              <component :is="sections.response" class="size-4" aria-hidden="true" />
              {{ t('security.sections.response') }}
            </legend>
            <SwitchField
              id="policy-limited"
              v-model="form.limited"
              :label="t('security.limitedResponse')"
              :hint="t('security.limitedResponseHint')"
            />
            <template v-if="form.limited">
              <div class="grid gap-4 sm:grid-cols-[8rem_1fr]">
                <FormField id="policy-limited-status" :label="t('security.status')">
                  <Input
                    id="policy-limited-status"
                    v-model="form.limitedStatus"
                    type="number"
                    min="400"
                    max="599"
                    required
                  />
                </FormField>
                <FormField id="policy-limited-type" :label="t('security.contentType')">
                  <Input
                    id="policy-limited-type"
                    v-model="form.limitedType"
                    class="font-mono text-xs"
                    placeholder="text/plain; charset=utf-8"
                    autocomplete="off"
                  />
                </FormField>
              </div>
              <FormField id="policy-limited-body" :label="t('security.body')">
                <Textarea id="policy-limited-body" v-model="form.limitedBody" rows="2" />
              </FormField>
            </template>
          </fieldset>
        </div>
        <SheetFooter>
          <Button type="submit" :disabled="put.isPending.value || invalid">
            <Save data-icon="inline-start" aria-hidden="true" />
            {{ t('common.save') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
