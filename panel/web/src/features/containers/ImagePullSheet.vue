<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch, type Component } from 'vue'
import {
  ArrowDownToLine,
  CheckCheck,
  CircleCheck,
  Clock,
  Download,
  Info,
  KeyRound,
  PackageOpen,
  ShieldCheck,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type {
  ImageLayerStateName,
  ImageLayerView,
  ImagePullMessage,
  LogTailError,
} from '@/api/generated'
import { pullImage } from '@/api/generated'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import CopyValue from '@/components/CopyValue.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Progress } from '@/components/ui/progress'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Switch } from '@/components/ui/switch'
import { commandHeaders, newIdempotencyKey } from '@/lib/command'
import { engineName } from '@/lib/containers'
import { formatters } from '@/lib/format'
import { finished, layersDone, PULL_TIMEOUT_MS, pullShare, shortId } from './presentation'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ engine: string }>()
const emit = defineEmits<{ pulled: [] }>()

const { t, locale } = useI18n()
const format = computed(() => formatters(locale.value))

const reference = ref('')
const platform = ref('')
const signIn = ref(false)
const username = ref('')
const password = ref('')

type Phase = 'idle' | 'pulling' | 'pulled' | 'failed'
const phase = ref<Phase>('idle')
const pulling = ref('')
const layers = ref<ImageLayerView[]>([])
const result = ref<Extract<ImagePullMessage, { kind: 'pulled' }>>()
/** Why the pull failed once it began. */
const ended = ref<LogTailError>()
/** Why the API would not pull, or could not be reached. */
const refusal = ref<unknown>()
let controller: AbortController | undefined

const icons: Record<ImageLayerStateName, Component> = {
  waiting: Clock,
  downloading: ArrowDownToLine,
  downloaded: ShieldCheck,
  extracting: PackageOpen,
  complete: CircleCheck,
  exists: CheckCheck,
}

const share = computed(() => pullShare(layers.value))
const status = computed(() => {
  switch (phase.value) {
    case 'pulling':
      return {
        tone: 'pending' as const,
        label: t('containers.pull.pulling', { image: pulling.value }),
      }
    case 'pulled':
      return {
        tone: 'positive' as const,
        label: result.value?.updated ? t('containers.pull.updated') : t('containers.pull.upToDate'),
      }
    case 'failed':
      return { tone: 'negative' as const, label: ended.value?.message ?? t('containers.pull.lost') }
    default:
      return undefined
  }
})

function bytes(layer: ImageLayerView): string {
  if (layer.total_bytes === 0 || (layer.state !== 'downloading' && layer.state !== 'extracting')) {
    return ''
  }
  return t('containers.pull.bytes', {
    current: format.value.bytes(Math.min(layer.current_bytes, layer.total_bytes)),
    total: format.value.bytes(layer.total_bytes),
  })
}

function layerShare(layer: ImageLayerView): number {
  return layer.total_bytes > 0
    ? (100 * Math.min(layer.current_bytes, layer.total_bytes)) / layer.total_bytes
    : 0
}

async function pull() {
  controller?.abort()
  const signal = (controller = new AbortController()).signal
  pulling.value = reference.value.trim()
  phase.value = 'pulling'
  layers.value = []
  result.value = undefined
  ended.value = undefined
  refusal.value = undefined
  let refused: unknown
  try {
    const { stream } = await pullImage({
      path: { engine: props.engine },
      body: {
        reference: pulling.value,
        platform: platform.value.trim() || undefined,
        credentials: signIn.value
          ? { username: username.value.trim(), password: password.value }
          : undefined,
      },
      headers: commandHeaders(newIdempotencyKey(), PULL_TIMEOUT_MS),
      signal,
      // A pull is started once; reconnecting would start another.
      sseMaxRetryAttempts: 1,
      fetch: async (input: RequestInfo | URL, init?: RequestInit) => {
        const response = await fetch(input, init)
        if (!response.ok) {
          refused = await response
            .clone()
            .json()
            .catch(() => undefined)
        }
        return response
      },
      onSseError: (error: unknown) => {
        refused ??= error
      },
    })
    for await (const message of stream) {
      if (message.kind === 'progress') {
        layers.value = message.layers
      } else if (message.kind === 'pulled') {
        layers.value = layers.value.map(finished)
        result.value = message
        phase.value = 'pulled'
        password.value = ''
        toast.success(t('containers.pull.done', { image: message.image.tags[0] ?? pulling.value }))
        emit('pulled')
      } else {
        ended.value = message.error
        phase.value = 'failed'
      }
    }
  } catch (error) {
    refused ??= error
  }
  if (phase.value === 'pulling' && !signal.aborted) {
    phase.value = 'failed'
    refusal.value = refused
  }
}

function reset() {
  controller?.abort()
  controller = undefined
  phase.value = 'idle'
  layers.value = []
  result.value = undefined
  ended.value = undefined
  refusal.value = undefined
  password.value = ''
}

watch(open, (opened) => {
  if (!opened) {
    reset()
  }
})
onBeforeUnmount(reset)
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="flex w-full flex-col gap-0 sm:max-w-xl">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2">
          <Download class="size-5 shrink-0" aria-hidden="true" />
          {{ t('containers.pull.title') }}
        </SheetTitle>
        <SheetDescription>
          {{ t('containers.pull.description', { engine: engineName(engine) }) }}
        </SheetDescription>
      </SheetHeader>

      <div class="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto px-4 pb-4">
        <form class="flex flex-col gap-4" @submit.prevent="pull">
          <div class="flex flex-col gap-2">
            <Label for="image-pull-reference">{{ t('containers.pull.image') }}</Label>
            <Input
              id="image-pull-reference"
              v-model="reference"
              class="font-mono"
              placeholder="nginx:1.27"
              autocomplete="off"
              autocapitalize="off"
              spellcheck="false"
              required
              aria-describedby="image-pull-reference-hint"
            />
            <p id="image-pull-reference-hint" class="text-muted-foreground text-xs">
              {{ t('containers.pull.imageHint') }}
            </p>
          </div>
          <div class="flex flex-col gap-2">
            <Label for="image-pull-platform">{{ t('containers.pull.platform') }}</Label>
            <Input
              id="image-pull-platform"
              v-model="platform"
              class="font-mono"
              placeholder="linux/arm64"
              autocomplete="off"
              autocapitalize="off"
              spellcheck="false"
              aria-describedby="image-pull-platform-hint"
            />
            <p id="image-pull-platform-hint" class="text-muted-foreground text-xs">
              {{ t('containers.pull.platformHint') }}
            </p>
          </div>
          <div class="flex items-center gap-2">
            <Switch id="image-pull-sign-in" v-model="signIn" />
            <Label for="image-pull-sign-in" class="flex items-center gap-2">
              <KeyRound class="size-4" aria-hidden="true" />
              {{ t('containers.pull.signIn') }}
            </Label>
          </div>
          <div v-if="signIn" class="grid gap-4 sm:grid-cols-2">
            <div class="flex flex-col gap-2">
              <Label for="image-pull-username">{{ t('containers.pull.username') }}</Label>
              <Input
                id="image-pull-username"
                v-model="username"
                autocomplete="off"
                autocapitalize="off"
                spellcheck="false"
                required
              />
            </div>
            <div class="flex flex-col gap-2">
              <Label for="image-pull-password">{{ t('containers.pull.password') }}</Label>
              <Input
                id="image-pull-password"
                v-model="password"
                type="password"
                autocomplete="off"
                required
              />
            </div>
            <p class="text-muted-foreground text-xs sm:col-span-2">
              {{ t('containers.pull.credentialsHint') }}
            </p>
          </div>
          <div>
            <Button type="submit" :disabled="phase === 'pulling' || !reference.trim()">
              <Download data-icon="inline-start" aria-hidden="true" />
              {{ t('containers.pull.pull') }}
            </Button>
          </div>
        </form>

        <ApiFailureAlert v-if="phase === 'failed' && refusal" :error="refusal" />
        <section
          v-else-if="phase !== 'idle'"
          class="flex flex-col gap-3"
          :aria-label="t('containers.pull.progress')"
        >
          <StatusIndicator v-if="status" :tone="status.tone" :label="status.label" />
          <Progress
            v-if="phase === 'pulling' || share !== undefined"
            :model-value="phase === 'pulled' ? 100 : Math.round(100 * (share ?? 0))"
            :aria-label="t('containers.pull.progress')"
          />
          <p v-if="layers.length" class="text-muted-foreground text-xs">
            {{
              t(
                'containers.pull.layersDone',
                { done: layersDone(layers), count: layers.length },
                layers.length,
              )
            }}
          </p>
          <ul v-if="layers.length" class="flex flex-col divide-y rounded-md border">
            <li
              v-for="layer in layers"
              :key="layer.id"
              class="flex flex-col gap-1.5 px-3 py-2 text-sm"
            >
              <span class="flex items-center gap-2">
                <component :is="icons[layer.state]" class="size-4 shrink-0" aria-hidden="true" />
                <span class="font-mono text-xs">{{ layer.id }}</span>
                <span class="text-muted-foreground">{{
                  t(`containers.pull.states.${layer.state}`)
                }}</span>
                <span class="text-muted-foreground ml-auto text-xs tabular-nums">{{
                  bytes(layer)
                }}</span>
              </span>
              <Progress
                v-if="bytes(layer)"
                class="h-1"
                :model-value="layerShare(layer)"
                :aria-label="t('containers.pull.layer', { layer: layer.id })"
              />
            </li>
          </ul>
          <dl v-if="result" class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
            <dt class="text-muted-foreground">{{ t('containers.images.tags') }}</dt>
            <dd class="font-mono text-xs break-all">{{ result.image.tags.join(', ') || '—' }}</dd>
            <dt class="text-muted-foreground">{{ t('containers.images.id') }}</dt>
            <dd class="font-mono text-xs">{{ shortId(result.image.id) }}</dd>
            <template v-if="result.digest">
              <dt class="text-muted-foreground">{{ t('containers.pull.digest') }}</dt>
              <dd class="min-w-0"><CopyValue :value="result.digest" /></dd>
            </template>
          </dl>
          <p
            v-if="phase === 'pulling'"
            class="text-muted-foreground flex items-start gap-2 text-xs"
          >
            <Info class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
            {{ t('containers.pull.keepsGoing') }}
          </p>
        </section>
      </div>
    </SheetContent>
  </Sheet>
</template>
