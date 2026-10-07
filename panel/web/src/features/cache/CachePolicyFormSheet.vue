<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { Filter, History, KeyRound, Plus, Save, Timer, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { CachePolicyView } from '@/api/generated'
import { putCachePolicyMutation } from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import SwitchField from '@/components/SwitchField.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import ConditionsEditor from '@/components/ConditionsEditor.vue'
import { RESOURCE_ID } from '@/lib/forms'
import {
  changeHeaders,
  notifyFailure,
  plainHeaders,
  useRefreshConfiguration,
} from '@/lib/configuration'
import {
  DEFAULT_KEY,
  cachePolicyBody,
  cachePolicyForm,
  policyProblems,
  type CachePolicyForm,
} from './forms'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ policy?: CachePolicyView; taken: readonly string[] }>()

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const put = useMutation(putCachePolicyMutation())

const form = reactive<CachePolicyForm>(cachePolicyForm())
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, cachePolicyForm(props.policy))
  }
})

const idError = computed(() => {
  if (props.policy || form.id === '') {
    return null
  }
  if (!RESOURCE_ID.test(form.id) || form.id === 'off') {
    return t('cache.idHint')
  }
  return props.taken.includes(form.id) ? t('cache.idTaken') : null
})
const problems = computed(() => policyProblems(form))
const invalid = computed(() => idError.value !== null || problems.value.length > 0)
const problem = (key: string) => problems.value.includes(key)

function submit() {
  const body = cachePolicyBody(form)
  put.mutate(
    {
      path: { id: body.id },
      body,
      headers: props.policy ? changeHeaders(props.policy.etag) : plainHeaders(),
    },
    {
      onSuccess: (saved) => {
        toast.success(t('cache.saved', { id: saved.id }))
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
    <SheetContent class="w-full overflow-y-auto sm:max-w-xl">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>{{ policy ? t('cache.edit') : t('cache.new') }}</SheetTitle>
          <SheetDescription>{{ t('cache.formHint') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-6 px-4">
          <FormField
            id="cache-policy-id"
            :label="t('cache.id')"
            :hint="idError ?? t('cache.idHint')"
          >
            <Input
              id="cache-policy-id"
              v-model="form.id"
              required
              :disabled="Boolean(policy)"
              :aria-invalid="idError !== null"
              class="font-mono text-xs"
              autocomplete="off"
            />
          </FormField>
          <SwitchField
            id="cache-policy-enabled"
            v-model="form.enabled"
            :label="t('cache.enabled')"
            :hint="t('cache.enabledHint')"
          />

          <fieldset class="flex flex-col gap-3">
            <legend class="mb-3 flex items-center gap-2 text-sm font-medium">
              <Timer class="size-4" aria-hidden="true" />
              {{ t('cache.sections.lifetime') }}
            </legend>
            <SwitchField
              id="cache-policy-origin"
              v-model="form.honorOrigin"
              :label="t('cache.honorOrigin')"
              :hint="t('cache.honorOriginHint')"
            />
            <FormField
              id="cache-policy-ttl"
              :label="t('cache.ttl')"
              :hint="problem('ttl') ? t('cache.problems.ttl') : t('cache.ttlHint')"
            >
              <Input
                id="cache-policy-ttl"
                v-model="form.ttl"
                :aria-invalid="problem('ttl')"
                class="w-32 font-mono text-xs"
                placeholder="10m"
                autocomplete="off"
              />
            </FormField>
            <div class="flex flex-col gap-2">
              <span class="text-sm font-medium">{{ t('cache.statusTtls') }}</span>
              <p class="text-muted-foreground text-xs">
                {{ problem('statuses') ? t('cache.problems.statuses') : t('cache.statusTtlsHint') }}
              </p>
              <div
                v-for="(row, index) in form.statusTtls"
                :key="index"
                class="flex items-end gap-2"
              >
                <FormField
                  :id="`cache-status-${index}`"
                  :label="t('cache.statuses')"
                  class="flex-1"
                >
                  <Input
                    :id="`cache-status-${index}`"
                    v-model="row.statuses"
                    class="font-mono text-xs"
                    placeholder="404 410"
                    autocomplete="off"
                  />
                </FormField>
                <FormField :id="`cache-status-ttl-${index}`" :label="t('cache.statusTtl')">
                  <Input
                    :id="`cache-status-ttl-${index}`"
                    v-model="row.ttl"
                    class="w-24 font-mono text-xs"
                    placeholder="1m"
                    autocomplete="off"
                  />
                </FormField>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  :aria-label="t('common.remove')"
                  @click="form.statusTtls.splice(index, 1)"
                >
                  <Trash2 aria-hidden="true" />
                </Button>
              </div>
              <div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  @click="form.statusTtls.push({ statuses: '', ttl: '' })"
                >
                  <Plus data-icon="inline-start" aria-hidden="true" />
                  {{ t('cache.addStatusTtl') }}
                </Button>
              </div>
            </div>
          </fieldset>

          <fieldset class="flex flex-col gap-3">
            <legend class="mb-3 flex items-center gap-2 text-sm font-medium">
              <KeyRound class="size-4" aria-hidden="true" />
              {{ t('cache.sections.key') }}
            </legend>
            <FormField id="cache-policy-key" :label="t('cache.key')" :hint="t('cache.keyHint')">
              <Input
                id="cache-policy-key"
                v-model="form.key"
                class="font-mono text-xs"
                :placeholder="DEFAULT_KEY"
                autocomplete="off"
              />
            </FormField>
            <FormField
              id="cache-policy-vary"
              :label="t('cache.vary')"
              :hint="problem('vary') ? t('cache.problems.vary') : t('cache.varyHint')"
            >
              <Input
                id="cache-policy-vary"
                v-model="form.vary"
                :aria-invalid="problem('vary')"
                class="font-mono text-xs"
                placeholder="accept-language"
                autocomplete="off"
              />
            </FormField>
          </fieldset>

          <fieldset class="flex flex-col gap-3">
            <legend class="mb-3 flex items-center gap-2 text-sm font-medium">
              <Filter class="size-4" aria-hidden="true" />
              {{ t('cache.sections.bypass') }}
            </legend>
            <p class="text-muted-foreground -mt-2 text-xs">
              {{ problem('bypass') ? t('cache.problems.bypass') : t('cache.bypassHint') }}
            </p>
            <ConditionsEditor v-model="form.bypass" id-prefix="cache-bypass" />
          </fieldset>

          <fieldset class="flex flex-col gap-3">
            <legend class="mb-3 flex items-center gap-2 text-sm font-medium">
              <History class="size-4" aria-hidden="true" />
              {{ t('cache.sections.stale') }}
            </legend>
            <p class="text-muted-foreground -mt-2 text-xs">
              {{ problem('stale') ? t('cache.problems.stale') : t('cache.staleHint') }}
            </p>
            <div class="grid gap-4 sm:grid-cols-2">
              <FormField id="cache-policy-swr" :label="t('cache.staleWhileRevalidate')">
                <Input
                  id="cache-policy-swr"
                  v-model="form.staleWhileRevalidate"
                  class="w-32 font-mono text-xs"
                  placeholder="30s"
                  autocomplete="off"
                />
              </FormField>
              <FormField id="cache-policy-sie" :label="t('cache.staleIfError')">
                <Input
                  id="cache-policy-sie"
                  v-model="form.staleIfError"
                  class="w-32 font-mono text-xs"
                  placeholder="5m"
                  autocomplete="off"
                />
              </FormField>
            </div>
          </fieldset>

          <FormField
            id="cache-policy-object"
            :label="t('cache.maxObjectSize')"
            :hint="
              problem('objectSize') ? t('cache.problems.objectSize') : t('cache.maxObjectSizeHint')
            "
          >
            <Input
              id="cache-policy-object"
              v-model="form.maxObjectSize"
              :aria-invalid="problem('objectSize')"
              class="w-32 font-mono text-xs"
              placeholder="8m"
              autocomplete="off"
            />
          </FormField>
          <SwitchField
            id="cache-policy-status"
            v-model="form.statusHeader"
            :label="t('cache.statusHeader')"
            :hint="t('cache.statusHeaderHint')"
          />
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
