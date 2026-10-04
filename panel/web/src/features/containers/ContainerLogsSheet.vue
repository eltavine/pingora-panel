<script setup lang="ts">
import { computed, nextTick, ref, useTemplateRef, watch } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import {
  CircleAlert,
  CircleStop,
  Download,
  Play,
  RefreshCw,
  Scissors,
  ScrollText,
  Search,
  SearchX,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type {
  ContainerLogLineView,
  ContainerLogStreamName,
  ContainerLogTailMessage,
  ContainerView,
} from '@/api/generated'
import { containerLogsOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import StatusIndicator, { type StatusTone } from '@/components/StatusIndicator.vue'
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
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { apiLocation } from '@/lib/api'
import { engineName } from '@/lib/containers'
import { downloadFile } from '@/lib/download'
import { useTail } from '@/lib/tail'
import {
  LOG_LIMIT,
  LOG_LINE_COUNTS,
  logFile,
  logTailUrl,
  matchesLine,
  plainText,
} from './presentation'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ engine: string; container: ContainerView }>()

const { t, d } = useI18n()
const STREAMS = ['all', 'stdout', 'stderr'] as const
/** How close to the end the view must be to keep up with new lines. */
const STICKY_PX = 32

const count = ref(String(LOG_LINE_COUNTS[1]))
const stream = ref<ContainerLogStreamName | 'all'>('all')
const search = ref('')
const timestamps = ref(true)
const name = computed(() => props.container.names[0] ?? props.container.id.slice(0, 12))

const logs = useQuery(
  computed(() => ({
    ...containerLogsOptions({
      path: { engine: props.engine, container: props.container.id },
      query: { lines: Number(count.value) },
    }),
    enabled: open.value,
    refetchOnWindowFocus: false,
  })),
)
const read = computed(() => logs.data.value?.lines ?? [])

const tail = useTail<ContainerLogLineView, ContainerLogTailMessage>(
  (after) =>
    new WebSocket(
      logTailUrl(
        props.engine,
        props.container.id,
        after ? { after } : { lines: '0' },
        apiLocation(),
      ),
    ),
  { items: (message) => message.lines, limit: LOG_LIMIT },
)
/** Whether the lines followed carry on from the lines read. */
const continued = ref(false)
const following = computed(() => tail.state.value === 'live' || tail.state.value === 'connecting')
const tailStatus = computed((): { tone: StatusTone; label: string } | undefined => {
  switch (tail.state.value) {
    case 'live':
      return { tone: 'positive', label: t('containers.logs.live') }
    case 'connecting':
      return { tone: 'pending', label: t('containers.logs.connecting') }
    case 'ended':
      return { tone: 'neutral', label: t('containers.logs.ended') }
    case 'failed':
      return { tone: 'negative', label: tail.failure.value?.message ?? t('containers.logs.lost') }
    default:
      return undefined
  }
})

/** Follows on from the last line read. */
function follow() {
  continued.value = true
  tail.start(read.value.at(-1)?.time)
}

function reread() {
  tail.stop()
  continued.value = false
  void logs.refetch()
}

watch(count, () => {
  tail.stop()
  continued.value = false
})
watch(open, (opened) => {
  if (!opened) {
    tail.stop()
    continued.value = false
  }
})

const lines = computed(() =>
  (continued.value ? [...read.value, ...tail.items.value] : read.value).slice(-LOG_LIMIT),
)
const shown = computed(() =>
  lines.value.filter((line) => matchesLine(line, stream.value, search.value)),
)

const viewport = useTemplateRef<HTMLElement>('viewport')
const sticky = ref(true)
function onScroll() {
  const element = viewport.value
  if (element) {
    sticky.value = element.scrollHeight - element.scrollTop - element.clientHeight < STICKY_PX
  }
}
watch(
  () => shown.value.length,
  async () => {
    if (!sticky.value) {
      return
    }
    await nextTick()
    const element = viewport.value
    if (element) {
      element.scrollTop = element.scrollHeight
    }
  },
)

function save() {
  const stamp = new Date().toISOString().replace(/[:.]/g, '-')
  downloadFile(`${name.value}-${stamp}.log`, logFile(shown.value), 'text/plain;charset=utf-8')
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="flex w-full flex-col gap-0 sm:max-w-4xl">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2 break-all">
          <ScrollText class="size-5 shrink-0" aria-hidden="true" />
          {{ t('containers.logs.title', { name }) }}
        </SheetTitle>
        <SheetDescription class="flex flex-wrap items-center gap-x-2">
          <span class="font-mono text-xs">{{ container.id.slice(0, 12) }}</span>
          <span aria-hidden="true">·</span>
          <span>{{ engineName(engine) }}</span>
          <span aria-hidden="true">·</span>
          <span>{{ t('containers.logs.secrets') }}</span>
        </SheetDescription>
      </SheetHeader>

      <div class="flex min-h-0 flex-1 flex-col gap-3 px-4 pb-4">
        <div class="grid grid-cols-2 gap-2 sm:flex sm:flex-wrap sm:items-center">
          <Select v-model="count">
            <SelectTrigger class="w-full sm:w-36" :aria-label="t('containers.logs.count')">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="option in LOG_LINE_COUNTS" :key="option" :value="String(option)">
                {{ t('containers.logs.last', { n: option }, option) }}
              </SelectItem>
            </SelectContent>
          </Select>
          <Select v-model="stream">
            <SelectTrigger class="w-full sm:w-40" :aria-label="t('containers.logs.stream')">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="option in STREAMS" :key="option" :value="option">
                {{ t(`containers.logs.streams.${option}`) }}
              </SelectItem>
            </SelectContent>
          </Select>
          <div class="relative col-span-2 sm:min-w-48 sm:flex-1">
            <Search
              class="text-muted-foreground absolute top-1/2 left-2.5 size-4 -translate-y-1/2"
              aria-hidden="true"
            />
            <Input
              v-model="search"
              type="search"
              class="pl-8"
              :placeholder="t('containers.logs.search')"
              :aria-label="t('containers.logs.search')"
            />
          </div>
        </div>
        <div class="flex flex-wrap items-center gap-2">
          <Button v-if="following" variant="outline" size="sm" @click="tail.stop()">
            <CircleStop data-icon="inline-start" aria-hidden="true" />
            {{ t('containers.logs.stop') }}
          </Button>
          <Button v-else size="sm" :disabled="!logs.data.value" @click="follow">
            <Play data-icon="inline-start" aria-hidden="true" />
            {{ t('containers.logs.follow') }}
          </Button>
          <Button variant="outline" size="sm" :disabled="logs.isFetching.value" @click="reread">
            <RefreshCw
              data-icon="inline-start"
              :class="{ 'animate-spin': logs.isFetching.value }"
              aria-hidden="true"
            />
            {{ t('state.refresh') }}
          </Button>
          <Button variant="outline" size="sm" :disabled="shown.length === 0" @click="save">
            <Download data-icon="inline-start" aria-hidden="true" />
            {{ t('containers.logs.download') }}
          </Button>
          <div class="ml-auto flex items-center gap-2">
            <Switch id="container-log-times" v-model="timestamps" />
            <Label for="container-log-times" class="text-sm">
              {{ t('containers.logs.times') }}
            </Label>
          </div>
        </div>

        <StatusIndicator v-if="tailStatus" :tone="tailStatus.tone" :label="tailStatus.label" />
        <p
          v-if="logs.data.value?.truncated"
          class="text-muted-foreground flex items-center gap-2 text-sm"
        >
          <Scissors class="size-4 shrink-0" aria-hidden="true" />
          {{ t('containers.logs.truncated') }}
        </p>

        <ApiFailureAlert
          v-if="logs.isError.value && !logs.data.value"
          :error="logs.error.value"
          retryable
          @retry="logs.refetch()"
        />
        <Skeleton
          v-else-if="logs.isPending.value"
          class="min-h-64 flex-1 rounded-md"
          aria-busy="true"
          :aria-label="t('state.loading')"
        />
        <div
          v-else
          ref="viewport"
          role="log"
          aria-live="off"
          tabindex="0"
          :aria-label="t('containers.logs.title', { name })"
          class="bg-muted/30 min-h-64 flex-1 overflow-auto rounded-md border py-2 font-mono text-xs leading-5"
          @scroll.passive="onScroll"
        >
          <div
            v-for="(line, index) in shown"
            :key="index"
            class="flex border-l-2 px-3 [contain-intrinsic-size:auto_1.25rem] [content-visibility:auto]"
            :class="[
              line.stream === 'stderr' ? 'border-foreground bg-muted' : 'border-transparent',
              timestamps ? 'flex-col sm:flex-row sm:gap-3' : 'gap-2',
            ]"
          >
            <span class="flex shrink-0 items-center gap-2">
              <time
                v-if="timestamps"
                class="text-muted-foreground tabular-nums"
                :datetime="line.time"
              >
                {{ d(new Date(line.time), 'precise') }}
              </time>
              <span class="flex h-5 w-3.5 items-center">
                <template v-if="line.stream === 'stderr'">
                  <CircleAlert class="size-3.5" aria-hidden="true" />
                  <span class="sr-only">{{ t('containers.logs.streams.stderr') }}</span>
                </template>
              </span>
            </span>
            <span class="min-w-0 whitespace-pre-wrap [overflow-wrap:anywhere]">{{
              plainText(line.text)
            }}</span>
          </div>
          <p
            v-if="shown.length === 0"
            class="text-muted-foreground flex items-center gap-2 px-3 font-sans text-sm"
          >
            <SearchX v-if="lines.length" class="size-4 shrink-0" aria-hidden="true" />
            <ScrollText v-else class="size-4 shrink-0" aria-hidden="true" />
            {{ lines.length ? t('containers.logs.noMatch') : t('containers.logs.empty') }}
          </p>
        </div>
        <p class="text-muted-foreground text-xs">
          {{
            t('containers.logs.shown', { shown: shown.length, count: lines.length }, lines.length)
          }}
        </p>
      </div>
    </SheetContent>
  </Sheet>
</template>
