<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import {
  Activity,
  Cable,
  Hash,
  HeartPulse,
  Info,
  Repeat,
  Save,
  Server,
  ShieldCheck,
  Shuffle,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { UpstreamView } from '@/api/generated'
import {
  createUpstreamMutation,
  replaceUpstreamMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ChoiceCards from '@/components/ChoiceCards.vue'
import FormField from '@/components/FormField.vue'
import SwitchField from '@/components/SwitchField.vue'
import { Button } from '@/components/ui/button'
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
import {
  ALGORITHMS,
  HEALTH_METHODS,
  HEALTH_PROTOCOLS,
  parseNodes,
  upstreamForm,
  upstreamInput,
  type Algorithm,
  type UpstreamForm,
} from './forms'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ upstream?: UpstreamView }>()
const emit = defineEmits<{ saved: [upstream: UpstreamView] }>()

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const create = useMutation(createUpstreamMutation())
const replace = useMutation(replaceUpstreamMutation())

const form = reactive<UpstreamForm>(upstreamForm())
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, upstreamForm(props.upstream))
  }
})

const algorithmIcons = { round_robin: Repeat, random: Shuffle, consistent_hash: Hash }
const algorithms = computed(() =>
  ALGORITHMS.map((value: Algorithm) => ({
    value,
    label: t(`upstreams.algorithms.${value}`),
    icon: algorithmIcons[value],
  })),
)
const invalidLines = computed(() => (props.upstream ? [] : parseNodes(form.nodes).invalid))
const busy = computed(() => create.isPending.value || replace.isPending.value)

function saved(upstream: UpstreamView) {
  toast.success(
    props.upstream ? t('common.saved') : t('upstreams.created', { name: upstream.name }),
  )
  open.value = false
  emit('saved', upstream)
  void refresh()
}

function submit() {
  const body = upstreamInput(form, props.upstream)
  const onError = (error: unknown) => notifyFailure(error, t('common.changeFailed'))
  if (props.upstream) {
    replace.mutate(
      { path: { id: props.upstream.id }, body, headers: changeHeaders(props.upstream.etag) },
      { onSuccess: saved, onError },
    )
  } else {
    create.mutate({ body, headers: plainHeaders() }, { onSuccess: saved, onError })
  }
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-2xl">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>{{ upstream ? t('upstreams.edit') : t('upstreams.new') }}</SheetTitle>
          <SheetDescription>{{ t('upstreams.description') }}</SheetDescription>
        </SheetHeader>

        <div class="flex flex-col gap-8 px-4">
          <fieldset class="flex flex-col gap-4">
            <legend class="mb-3 flex items-center gap-2 font-medium">
              <Info class="size-4" aria-hidden="true" />
              {{ t('upstreams.general') }}
            </legend>
            <FormField id="upstream-name" :label="t('common.name')">
              <Input
                id="upstream-name"
                v-model="form.name"
                required
                maxlength="128"
                autocomplete="off"
              />
            </FormField>
            <div class="flex flex-col gap-1.5">
              <Label>{{ t('upstreams.balancing') }}</Label>
              <ChoiceCards
                v-model="form.algorithm"
                :label="t('upstreams.balancing')"
                :choices="algorithms"
              />
            </div>
            <FormField
              v-if="form.algorithm === 'consistent_hash'"
              id="upstream-hash-key"
              :label="t('upstreams.hashKey')"
              :hint="t('upstreams.hashKeyHint')"
            >
              <Input
                id="upstream-hash-key"
                v-model="form.hashKey"
                required
                class="font-mono text-xs"
                autocomplete="off"
              />
            </FormField>
            <FormField
              id="upstream-host"
              :label="t('upstreams.hostHeader')"
              :hint="t('upstreams.hostHeaderHint')"
            >
              <Input id="upstream-host" v-model="form.hostHeader" autocomplete="off" />
            </FormField>
            <FormField id="upstream-note" :label="t('common.note')">
              <Textarea id="upstream-note" v-model="form.note" rows="2" />
            </FormField>
          </fieldset>

          <fieldset v-if="!upstream" class="flex flex-col gap-2">
            <legend class="mb-3 flex items-center gap-2 font-medium">
              <Server class="size-4" aria-hidden="true" />
              {{ t('upstreams.nodes') }}
            </legend>
            <Textarea
              id="upstream-nodes"
              v-model="form.nodes"
              rows="4"
              class="font-mono text-xs"
              placeholder="10.0.0.11:8080 weight=3&#10;10.0.0.12:8080&#10;10.0.0.13:8080 backup"
              :aria-label="t('upstreams.nodes')"
              :aria-invalid="invalidLines.length > 0"
            />
            <p class="text-muted-foreground text-xs">{{ t('upstreams.nodesHint') }}</p>
            <p v-if="invalidLines.length > 0" class="text-destructive text-xs" role="alert">
              {{ t('upstreams.invalidNodeLines', { lines: invalidLines.join(', ') }) }}
            </p>
          </fieldset>

          <fieldset class="flex flex-col gap-4">
            <legend class="mb-3 flex items-center gap-2 font-medium">
              <Cable class="size-4" aria-hidden="true" />
              {{ t('upstreams.connection.title') }}
            </legend>
            <div class="grid gap-4 sm:grid-cols-2">
              <FormField id="upstream-connect" :label="t('upstreams.connection.connectTimeout')">
                <Input
                  id="upstream-connect"
                  v-model.number="form.connectTimeout"
                  type="number"
                  min="1"
                />
              </FormField>
              <FormField id="upstream-read" :label="t('upstreams.connection.readTimeout')">
                <Input id="upstream-read" v-model.number="form.readTimeout" type="number" min="1" />
              </FormField>
              <FormField id="upstream-write" :label="t('upstreams.connection.writeTimeout')">
                <Input
                  id="upstream-write"
                  v-model.number="form.writeTimeout"
                  type="number"
                  min="1"
                />
              </FormField>
              <FormField id="upstream-idle" :label="t('upstreams.connection.idleTimeout')">
                <Input id="upstream-idle" v-model.number="form.idleTimeout" type="number" min="1" />
              </FormField>
              <FormField
                id="upstream-max"
                :label="t('upstreams.connection.maxConnections')"
                :hint="t('upstreams.connection.maxConnectionsHint')"
              >
                <Input
                  id="upstream-max"
                  v-model.number="form.maxConnections"
                  type="number"
                  min="1"
                />
              </FormField>
            </div>
            <SwitchField
              id="upstream-keepalive"
              v-model="form.keepalive"
              :label="t('upstreams.connection.keepalive')"
            />
            <SwitchField
              id="upstream-http2"
              v-model="form.http2"
              :label="t('upstreams.connection.http2')"
            />
          </fieldset>

          <fieldset class="flex flex-col gap-4">
            <legend class="mb-3 flex items-center gap-2 font-medium">
              <ShieldCheck class="size-4" aria-hidden="true" />
              {{ t('upstreams.tls.title') }}
            </legend>
            <SwitchField
              id="upstream-verify-cert"
              v-model="form.verifyCertificate"
              :label="t('upstreams.tls.verifyCertificate')"
            />
            <SwitchField
              id="upstream-verify-host"
              v-model="form.verifyHostname"
              :label="t('upstreams.tls.verifyHostname')"
              :disabled="!form.verifyCertificate"
            />
            <div class="grid gap-4 sm:grid-cols-2">
              <FormField id="upstream-sni" :label="t('upstreams.tls.sni')">
                <Input id="upstream-sni" v-model="form.sni" autocomplete="off" />
              </FormField>
              <FormField
                id="upstream-ca"
                :label="t('upstreams.tls.ca')"
                :hint="t('upstreams.tls.caHint')"
              >
                <Input
                  id="upstream-ca"
                  v-model="form.caSecretId"
                  class="font-mono text-xs"
                  autocomplete="off"
                />
              </FormField>
            </div>
          </fieldset>

          <fieldset class="flex flex-col gap-4">
            <legend class="mb-3 flex items-center gap-2 font-medium">
              <HeartPulse class="size-4" aria-hidden="true" />
              {{ t('upstreams.health.title') }}
            </legend>
            <SwitchField
              id="upstream-health"
              v-model="form.healthEnabled"
              :label="t('upstreams.health.enabled')"
            />
            <div v-if="form.healthEnabled" class="grid gap-4 sm:grid-cols-2">
              <FormField id="health-protocol" :label="t('upstreams.health.protocol')">
                <Select v-model="form.healthProtocol">
                  <SelectTrigger id="health-protocol" class="w-full"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem v-for="value in HEALTH_PROTOCOLS" :key="value" :value="value">
                      {{ value.toUpperCase() }}
                    </SelectItem>
                  </SelectContent>
                </Select>
              </FormField>
              <template v-if="form.healthProtocol === 'http'">
                <FormField id="health-method" :label="t('upstreams.health.method')">
                  <Select v-model="form.healthMethod">
                    <SelectTrigger id="health-method" class="w-full"><SelectValue /></SelectTrigger>
                    <SelectContent>
                      <SelectItem v-for="value in HEALTH_METHODS" :key="value" :value="value">
                        {{ value }}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </FormField>
                <FormField id="health-path" :label="t('upstreams.health.path')">
                  <Input
                    id="health-path"
                    v-model="form.healthPath"
                    class="font-mono text-xs"
                    autocomplete="off"
                  />
                </FormField>
                <FormField id="health-host" :label="t('upstreams.health.host')">
                  <Input id="health-host" v-model="form.healthHost" autocomplete="off" />
                </FormField>
                <FormField
                  id="health-statuses"
                  :label="t('upstreams.health.statuses')"
                  :hint="t('upstreams.health.statusesHint')"
                >
                  <Input
                    id="health-statuses"
                    v-model="form.expectedStatuses"
                    placeholder="200, 204"
                    autocomplete="off"
                  />
                </FormField>
              </template>
              <FormField id="health-interval" :label="t('upstreams.health.interval')">
                <Input
                  id="health-interval"
                  v-model.number="form.healthInterval"
                  type="number"
                  min="100"
                />
              </FormField>
              <FormField id="health-timeout" :label="t('upstreams.health.timeout')">
                <Input
                  id="health-timeout"
                  v-model.number="form.healthTimeout"
                  type="number"
                  min="100"
                />
              </FormField>
              <FormField id="health-healthy" :label="t('upstreams.health.healthyThreshold')">
                <Input
                  id="health-healthy"
                  v-model.number="form.healthyThreshold"
                  type="number"
                  min="1"
                />
              </FormField>
              <FormField id="health-unhealthy" :label="t('upstreams.health.unhealthyThreshold')">
                <Input
                  id="health-unhealthy"
                  v-model.number="form.unhealthyThreshold"
                  type="number"
                  min="1"
                />
              </FormField>
            </div>
          </fieldset>

          <fieldset class="flex flex-col gap-4">
            <legend class="mb-3 flex items-center gap-2 font-medium">
              <Activity class="size-4" aria-hidden="true" />
              {{ t('upstreams.passive.title') }}
            </legend>
            <SwitchField
              id="upstream-passive"
              v-model="form.passiveEnabled"
              :label="t('upstreams.passive.enabled')"
            />
            <div v-if="form.passiveEnabled" class="grid gap-4 sm:grid-cols-2">
              <FormField id="passive-failures" :label="t('upstreams.passive.failureThreshold')">
                <Input
                  id="passive-failures"
                  v-model.number="form.failureThreshold"
                  type="number"
                  min="1"
                />
              </FormField>
              <FormField id="passive-ejection" :label="t('upstreams.passive.ejection')">
                <Input
                  id="passive-ejection"
                  v-model.number="form.ejection"
                  type="number"
                  min="100"
                />
              </FormField>
            </div>
          </fieldset>
        </div>

        <SheetFooter>
          <Button type="submit" :disabled="busy || invalidLines.length > 0">
            <Save data-icon="inline-start" aria-hidden="true" />
            {{ upstream ? t('common.save') : t('common.create') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
