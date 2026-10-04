<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Save } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { AlertRuleView } from '@/api/generated'
import {
  listAlertChannelsOptions,
  listSitesOptions,
  listUpstreamsOptions,
  putAlertRuleMutation,
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
import { Textarea } from '@/components/ui/textarea'
import { changeHeaders, notifyFailure, plainHeaders } from '@/lib/configuration'
import {
  isRatio,
  MEASURES,
  PENDING_CHOICES,
  readsRequests,
  ruleBody,
  ruleForm,
  type RuleForm,
} from './presentation'

const EVERY = '*'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ rule?: AlertRuleView }>()
const emit = defineEmits<{ saved: [] }>()

const { t } = useI18n()
const put = useMutation(putAlertRuleMutation())
const sites = useQuery(
  computed(() => ({ ...listSitesOptions({ query: { limit: 500 } }), enabled: open.value })),
)
const upstreams = useQuery(computed(() => ({ ...listUpstreamsOptions(), enabled: open.value })))
const channels = useQuery(computed(() => ({ ...listAlertChannelsOptions(), enabled: open.value })))

const form = reactive<RuleForm>(ruleForm())
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, ruleForm(props.rule))
  }
})

const unit = computed(() => {
  if (isRatio(form.measure)) {
    return '%'
  }
  return form.measure === 'latency_p95' ? 's' : form.measure === 'request_rate' ? 'req/s' : ''
})

const site = computed({
  get: () => form.site || EVERY,
  set: (value: string) => {
    form.site = value === EVERY ? '' : value
    if (!form.site) {
      form.route = ''
    }
  },
})
const upstream = computed({
  get: () => form.upstream || EVERY,
  set: (value: string) => (form.upstream = value === EVERY ? '' : value),
})
const pending = computed({
  get: () => String(form.pendingSeconds),
  set: (value: string) => (form.pendingSeconds = Number(value)),
})

function toggleChannel(id: string, checked: boolean | 'indeterminate') {
  form.channels =
    checked === true
      ? [...new Set([...form.channels, id])]
      : form.channels.filter((channel) => channel !== id)
}

function pendingLabel(seconds: number): string {
  return seconds === 0
    ? t('alerts.atOnce')
    : seconds < 3_600
      ? t('alerts.minutes', { n: seconds / 60 }, seconds / 60)
      : t('alerts.hours', { n: seconds / 3_600 }, seconds / 3_600)
}

function submit() {
  const id = props.rule?.id ?? form.id.trim()
  put.mutate(
    {
      path: { id },
      body: ruleBody(form),
      headers: props.rule ? changeHeaders(props.rule.etag) : plainHeaders(),
    },
    {
      onSuccess: () => {
        toast.success(props.rule ? t('common.saved') : t('alerts.ruleCreated'))
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
    <SheetContent class="w-full overflow-y-auto sm:max-w-xl">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>{{ rule ? t('alerts.editRuleTitle') : t('alerts.newRule') }}</SheetTitle>
          <SheetDescription>{{ t('alerts.ruleDescription') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-4 px-4">
          <div class="grid gap-4 sm:grid-cols-2">
            <FormField v-if="!rule" id="alert-rule-id" :label="t('alerts.ruleId')">
              <Input
                id="alert-rule-id"
                v-model="form.id"
                required
                maxlength="64"
                pattern="[A-Za-z0-9._\-]+"
                class="font-mono"
                autocomplete="off"
              />
            </FormField>
            <FormField id="alert-rule-name" :label="t('alerts.ruleName')">
              <Input
                id="alert-rule-name"
                v-model="form.name"
                maxlength="128"
                :placeholder="rule?.id ?? form.id"
                autocomplete="off"
              />
            </FormField>
          </div>

          <FormField id="alert-rule-measure" :label="t('alerts.measure')">
            <Select v-model="form.measure">
              <SelectTrigger id="alert-rule-measure" class="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem v-for="measure in MEASURES" :key="measure" :value="measure">
                  {{ t(`alerts.measures.${measure}`) }}
                </SelectItem>
              </SelectContent>
            </Select>
          </FormField>

          <div class="grid gap-4 sm:grid-cols-[1fr_1fr_1fr]">
            <FormField id="alert-rule-comparison" :label="t('alerts.comparison')">
              <Select v-model="form.comparison">
                <SelectTrigger id="alert-rule-comparison" class="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="above">{{ t('alerts.above') }}</SelectItem>
                  <SelectItem value="below">{{ t('alerts.below') }}</SelectItem>
                </SelectContent>
              </Select>
            </FormField>
            <FormField id="alert-rule-threshold" :label="t('alerts.threshold')">
              <div class="relative">
                <Input
                  id="alert-rule-threshold"
                  v-model="form.threshold"
                  type="number"
                  min="0"
                  :max="isRatio(form.measure) ? 100 : undefined"
                  step="any"
                  required
                  :class="unit ? 'pr-14' : undefined"
                />
                <span
                  v-if="unit"
                  class="text-muted-foreground pointer-events-none absolute top-1/2 right-3 -translate-y-1/2 text-sm"
                  aria-hidden="true"
                  >{{ unit }}</span
                >
              </div>
            </FormField>
            <FormField id="alert-rule-pending" :label="t('alerts.pending')">
              <Select v-model="pending">
                <SelectTrigger id="alert-rule-pending" class="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem
                    v-for="seconds in PENDING_CHOICES"
                    :key="seconds"
                    :value="String(seconds)"
                  >
                    {{ pendingLabel(seconds) }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </FormField>
          </div>

          <div v-if="readsRequests(form.measure)" class="grid gap-4 sm:grid-cols-2">
            <FormField id="alert-rule-site" :label="t('alerts.site')">
              <Select v-model="site">
                <SelectTrigger id="alert-rule-site" class="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem :value="EVERY">{{ t('alerts.everySite') }}</SelectItem>
                  <SelectItem
                    v-for="item in sites.data.value?.items ?? []"
                    :key="item.id"
                    :value="item.id"
                  >
                    {{ item.name }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </FormField>
            <FormField id="alert-rule-route" :label="t('alerts.route')">
              <Input
                id="alert-rule-route"
                v-model="form.route"
                :disabled="!form.site"
                class="font-mono"
                autocomplete="off"
              />
            </FormField>
          </div>
          <FormField
            v-else-if="form.measure === 'upstream_error_ratio'"
            id="alert-rule-upstream"
            :label="t('alerts.upstream')"
          >
            <Select v-model="upstream">
              <SelectTrigger id="alert-rule-upstream" class="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem :value="EVERY">{{ t('alerts.everyUpstream') }}</SelectItem>
                <SelectItem
                  v-for="item in upstreams.data.value ?? []"
                  :key="item.id"
                  :value="item.id"
                >
                  {{ item.name }}
                </SelectItem>
              </SelectContent>
            </Select>
          </FormField>

          <FormField id="alert-rule-severity" :label="t('alerts.severity')">
            <Select v-model="form.severity">
              <SelectTrigger id="alert-rule-severity" class="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="warning">{{ t('alerts.severities.warning') }}</SelectItem>
                <SelectItem value="critical">{{ t('alerts.severities.critical') }}</SelectItem>
              </SelectContent>
            </Select>
          </FormField>

          <fieldset class="flex flex-col gap-2">
            <legend class="mb-1.5 text-sm font-medium">{{ t('alerts.channels') }}</legend>
            <label
              v-for="channel in channels.data.value ?? []"
              :key="channel.id"
              class="flex items-center gap-2 text-sm"
            >
              <Checkbox
                :model-value="form.channels.includes(channel.id)"
                @update:model-value="toggleChannel(channel.id, $event)"
              />
              <span class="font-mono text-xs">{{ channel.id }}</span>
              <span class="text-muted-foreground truncate text-xs">{{ channel.target }}</span>
            </label>
            <p class="text-muted-foreground text-xs">{{ t('alerts.channelsHint') }}</p>
          </fieldset>

          <FormField id="alert-rule-description" :label="t('alerts.ruleNote')">
            <Textarea id="alert-rule-description" v-model="form.description" rows="2" />
          </FormField>

          <SwitchField
            id="alert-rule-enabled"
            v-model="form.enabled"
            :label="t('common.enabled')"
          />
        </div>
        <SheetFooter>
          <Button type="submit" :disabled="put.isPending.value">
            <Save data-icon="inline-start" aria-hidden="true" />
            {{ rule ? t('common.save') : t('common.create') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
