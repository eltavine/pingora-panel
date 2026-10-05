<script setup lang="ts">
import { computed, reactive, watch, type Component } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { ArrowDownToLine, ArrowUpFromLine, Globe, Save, Server, Shrink } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { CompressionAlgorithm, HttpPolicyView, ServerHeader } from '@/api/generated'
import { putHttpPolicyMutation } from '@/api/generated/@tanstack/vue-query.gen'
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
import { RESOURCE_ID, parseSize } from '@/lib/forms'
import {
  changeHeaders,
  notifyFailure,
  plainHeaders,
  useRefreshConfiguration,
} from '@/lib/configuration'
import FieldChangesEditor from './FieldChangesEditor.vue'
import {
  CODINGS,
  SERVER_MODES,
  fieldNameInvalid,
  httpPolicyBody,
  httpPolicyForm,
  type HttpPolicyForm,
} from './forms'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ policy?: HttpPolicyView; taken: readonly string[] }>()

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const put = useMutation(putHttpPolicyMutation())

const form = reactive<HttpPolicyForm>(httpPolicyForm())
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, httpPolicyForm(props.policy))
  }
})

const idError = computed(() => {
  if (props.policy || form.id === '') {
    return null
  }
  if (!RESOURCE_ID.test(form.id)) {
    return t('httpPolicies.idHint')
  }
  return props.taken.includes(form.id) ? t('httpPolicies.idTaken') : null
})
const minSizeInvalid = computed(() => form.compression && Number.isNaN(parseSize(form.minSize)))
const invalid = computed(
  () =>
    idError.value !== null ||
    minSizeInvalid.value ||
    [...form.request, ...form.response].some(fieldNameInvalid) ||
    (form.compression && form.algorithms.length === 0),
)

function toggleCoding(algorithm: CompressionAlgorithm, checked: boolean | 'indeterminate') {
  form.algorithms =
    checked === true
      ? [...form.algorithms.filter((value) => value !== algorithm), algorithm]
      : form.algorithms.filter((value) => value !== algorithm)
}

function setServer(value: unknown) {
  form.server = value as ServerHeader['mode']
}

function submit() {
  const body = httpPolicyBody(form)
  put.mutate(
    {
      path: { id: body.id },
      body,
      headers: props.policy ? changeHeaders(props.policy.etag) : plainHeaders(),
    },
    {
      onSuccess: (saved) => {
        toast.success(t('httpPolicies.saved', { id: saved.id }))
        open.value = false
        void refresh()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

const sections: Record<string, Component> = {
  request: ArrowUpFromLine,
  response: ArrowDownToLine,
  server: Server,
  cors: Globe,
  compression: Shrink,
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-xl">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>{{ policy ? t('httpPolicies.edit') : t('httpPolicies.new') }}</SheetTitle>
          <SheetDescription>{{ t('httpPolicies.description') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-6 px-4">
          <FormField
            id="http-policy-id"
            :label="t('httpPolicies.id')"
            :hint="idError ?? t('httpPolicies.idHint')"
          >
            <Input
              id="http-policy-id"
              v-model="form.id"
              required
              :disabled="Boolean(policy)"
              :aria-invalid="idError !== null"
              class="font-mono text-xs"
              autocomplete="off"
            />
          </FormField>

          <fieldset class="flex flex-col gap-3">
            <legend class="mb-3 flex items-center gap-2 text-sm font-medium">
              <component :is="sections.request" class="size-4" aria-hidden="true" />
              {{ t('httpPolicies.sections.request') }}
            </legend>
            <p class="text-muted-foreground -mt-2 text-xs">{{ t('httpPolicies.requestHint') }}</p>
            <FieldChangesEditor
              v-model="form.request"
              id-prefix="http-policy-request"
              value-placeholder="$host"
            />
          </fieldset>

          <fieldset class="flex flex-col gap-3">
            <legend class="mb-3 flex items-center gap-2 text-sm font-medium">
              <component :is="sections.response" class="size-4" aria-hidden="true" />
              {{ t('httpPolicies.sections.response') }}
            </legend>
            <p class="text-muted-foreground -mt-2 text-xs">{{ t('httpPolicies.responseHint') }}</p>
            <FieldChangesEditor
              v-model="form.response"
              id-prefix="http-policy-response"
              value-placeholder="DENY"
            />
          </fieldset>

          <fieldset class="flex flex-col gap-3">
            <legend class="mb-3 flex items-center gap-2 text-sm font-medium">
              <component :is="sections.server" class="size-4" aria-hidden="true" />
              {{ t('httpPolicies.sections.server') }}
            </legend>
            <div class="grid gap-4 sm:grid-cols-2">
              <FormField id="http-policy-server" :label="t('httpPolicies.server')">
                <Select :model-value="form.server" @update:model-value="setServer($event)">
                  <SelectTrigger id="http-policy-server" class="w-full"
                    ><SelectValue
                  /></SelectTrigger>
                  <SelectContent>
                    <SelectItem v-for="mode in SERVER_MODES" :key="mode" :value="mode">
                      {{ t(`httpPolicies.serverModes.${mode}`) }}
                    </SelectItem>
                  </SelectContent>
                </Select>
              </FormField>
              <FormField
                v-if="form.server === 'replace'"
                id="http-policy-server-value"
                :label="t('httpPolicies.serverValue')"
              >
                <Input
                  id="http-policy-server-value"
                  v-model="form.serverValue"
                  required
                  autocomplete="off"
                />
              </FormField>
            </div>
          </fieldset>

          <fieldset class="flex flex-col gap-3">
            <legend class="mb-3 flex items-center gap-2 text-sm font-medium">
              <component :is="sections.cors" class="size-4" aria-hidden="true" />
              {{ t('httpPolicies.sections.cors') }}
            </legend>
            <SwitchField
              id="http-policy-cors"
              v-model="form.cors"
              :label="t('httpPolicies.cors')"
              :hint="t('httpPolicies.corsHint')"
            />
            <template v-if="form.cors">
              <FormField
                id="http-policy-origins"
                :label="t('httpPolicies.origins')"
                :hint="t('httpPolicies.originsHint')"
              >
                <Textarea
                  id="http-policy-origins"
                  v-model="form.origins"
                  rows="2"
                  required
                  class="font-mono text-xs"
                  placeholder="https://shop.example&#10;https://*.shop.example"
                />
              </FormField>
              <FormField
                id="http-policy-methods"
                :label="t('httpPolicies.methods')"
                :hint="t('httpPolicies.methodsHint')"
              >
                <Input
                  id="http-policy-methods"
                  v-model="form.methods"
                  class="font-mono text-xs"
                  placeholder="PUT DELETE"
                  autocomplete="off"
                />
              </FormField>
              <div class="grid gap-4 sm:grid-cols-2">
                <FormField
                  id="http-policy-headers"
                  :label="t('httpPolicies.headers')"
                  :hint="t('httpPolicies.headersHint')"
                >
                  <Input
                    id="http-policy-headers"
                    v-model="form.headers"
                    class="font-mono text-xs"
                    placeholder="X-Api-Key"
                    autocomplete="off"
                  />
                </FormField>
                <FormField
                  id="http-policy-expose"
                  :label="t('httpPolicies.expose')"
                  :hint="t('httpPolicies.exposeHint')"
                >
                  <Input
                    id="http-policy-expose"
                    v-model="form.expose"
                    class="font-mono text-xs"
                    placeholder="X-Request-Id"
                    autocomplete="off"
                  />
                </FormField>
              </div>
              <SwitchField
                id="http-policy-credentials"
                v-model="form.credentials"
                :label="t('httpPolicies.credentials')"
                :hint="t('httpPolicies.credentialsHint')"
              />
              <FormField
                id="http-policy-max-age"
                :label="t('httpPolicies.maxAge')"
                :hint="t('httpPolicies.maxAgeHint')"
              >
                <Input
                  id="http-policy-max-age"
                  v-model="form.maxAge"
                  type="number"
                  min="0"
                  max="86400"
                  class="w-32"
                />
              </FormField>
            </template>
          </fieldset>

          <fieldset class="flex flex-col gap-3">
            <legend class="mb-3 flex items-center gap-2 text-sm font-medium">
              <component :is="sections.compression" class="size-4" aria-hidden="true" />
              {{ t('httpPolicies.sections.compression') }}
            </legend>
            <SwitchField
              id="http-policy-compression"
              v-model="form.compression"
              :label="t('httpPolicies.compression')"
              :hint="t('httpPolicies.compressionHint')"
            />
            <template v-if="form.compression">
              <div class="flex flex-col gap-2">
                <span class="text-sm font-medium">{{ t('httpPolicies.codings') }}</span>
                <div class="flex flex-wrap gap-x-4 gap-y-2">
                  <label
                    v-for="coding in CODINGS"
                    :key="coding.algorithm"
                    class="flex items-center gap-2 font-mono text-xs"
                  >
                    <Checkbox
                      :model-value="form.algorithms.includes(coding.algorithm)"
                      @update:model-value="toggleCoding(coding.algorithm, $event)"
                    />
                    {{ coding.name }}
                  </label>
                </div>
                <p class="text-muted-foreground text-xs">{{ t('httpPolicies.codingsHint') }}</p>
              </div>
              <FormField
                id="http-policy-types"
                :label="t('httpPolicies.types')"
                :hint="t('httpPolicies.typesHint')"
              >
                <Textarea
                  id="http-policy-types"
                  v-model="form.types"
                  rows="3"
                  class="font-mono text-xs"
                />
              </FormField>
              <FormField
                id="http-policy-min-size"
                :label="t('httpPolicies.minSize')"
                :hint="
                  minSizeInvalid ? t('httpPolicies.invalidSize') : t('httpPolicies.minSizeHint')
                "
              >
                <Input
                  id="http-policy-min-size"
                  v-model="form.minSize"
                  :aria-invalid="minSizeInvalid"
                  class="w-32 font-mono text-xs"
                  placeholder="1k"
                  autocomplete="off"
                />
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
