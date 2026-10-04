<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useInfiniteQuery, useMutation, useQueryClient } from '@tanstack/vue-query'
import { watchDebounced } from '@vueuse/core'
import {
  Download,
  FilterX,
  History,
  ListFilter,
  Logs,
  Pause,
  Radio,
  RotateCw,
  Search,
  Trash2,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { toast } from 'vue-sonner'
import {
  searchLogs,
  type LogKindName,
  type LogPageResponse,
  type LogRecordItem,
  type LogTailMessage,
} from '@/api/generated'
import {
  deleteLogsMutation,
  listLogDeletionsQueryKey,
  searchLogsQueryKey,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Spinner } from '@/components/ui/spinner'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { apiLocation } from '@/lib/api'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { useSession } from '@/lib/session'
import { useTail } from '@/lib/tail'
import LogDeletionsSheet from './LogDeletionsSheet.vue'
import LogRecordSheet from './LogRecordSheet.vue'
import LogStatus from './LogStatus.vue'
import { downloadUrl, FILTERS, queryOf, TAIL_LIMIT, tailUrl } from './presentation'
import { summaryOf } from '@/lib/logs'

const PAGE_SIZE = 100
const ALL = 'all'
const FIELDS = [...FILTERS, 'since', 'until'] as const
type Field = (typeof FIELDS)[number]
/** Filters behind "More filters", which opens by itself when one is set. */
const MORE: readonly Field[] = ['route', 'client', 'path', 'request_id', 'since', 'until']

const { t, d } = useI18n()
const route = useRoute()
const router = useRouter()
const { can } = useSession()
const queryClient = useQueryClient()

function queried(field: Field): string {
  const value = route.query[field]
  return typeof value === 'string' ? value : ''
}

/** Filters as typed; they reach the URL, and the query, after a pause. */
const draft = ref<Record<Field, string>>(
  Object.fromEntries(FIELDS.map((field) => [field, queried(field)])) as Record<Field, string>,
)
watch(
  () => route.query,
  () => {
    for (const field of FIELDS) {
      if (queried(field) !== draft.value[field]) {
        draft.value[field] = queried(field)
      }
    }
  },
)
watchDebounced(
  draft,
  (values) => {
    const query = { ...route.query }
    for (const field of FIELDS) {
      if (values[field]) {
        query[field] = values[field]
      } else {
        delete query[field]
      }
    }
    void router.replace({ query })
  },
  { debounce: 300, deep: true },
)

const kind = computed({
  get: () => draft.value.kind || ALL,
  set: (value: string) => (draft.value.kind = value === ALL ? '' : value),
})
const moreOpen = ref(MORE.some((field) => queried(field)))
const filtered = computed(() => FIELDS.some((field) => queried(field)))

function clear() {
  for (const field of FIELDS) {
    draft.value[field] = ''
  }
}

/** `datetime-local` values in the browser's zone, as RFC 3339. */
function instant(value: string): string | undefined {
  const time = value ? new Date(value) : undefined
  return time && !Number.isNaN(time.getTime()) ? time.toISOString() : undefined
}

const filters = computed(() =>
  queryOf(Object.fromEntries(FILTERS.map((field) => [field, queried(field)]))),
)
const search = computed(() => ({
  ...filters.value,
  kind: filters.value.kind as LogKindName | undefined,
  since: instant(queried('since')),
  until: instant(queried('until')),
}))

const pages = useInfiniteQuery(
  computed(() => ({
    queryKey: searchLogsQueryKey({ query: search.value }),
    queryFn: async ({ pageParam, signal }: { pageParam?: string; signal: AbortSignal }) => {
      const query = { ...search.value, limit: PAGE_SIZE, until: pageParam ?? search.value.until }
      const { data } = await searchLogs({ query, signal, throwOnError: true })
      return data
    },
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (page: LogPageResponse) => page.next_until ?? undefined,
  })),
)
const items = computed(() => pages.data.value?.pages.flatMap((page) => page.records) ?? [])

const tail = useTail<LogRecordItem, LogTailMessage>(
  (after) =>
    new WebSocket(tailUrl({ ...filters.value, ...(after ? { after } : {}) }, apiLocation())),
  { items: (message) => message.records, limit: TAIL_LIMIT, newestFirst: true },
)
const following = computed(() => tail.state.value !== 'idle')
const tailFailed = computed(() => tail.state.value === 'failed')

/** Follows on from the newest record shown when it is the latest there is. */
function follow() {
  tail.start(queried('until') ? undefined : items.value[0]?.time)
}
watch(filters, () => {
  if (following.value && !tailFailed.value) {
    tail.start()
  }
})

const rows = computed(() => [...tail.items.value, ...items.value])
const keys = new WeakMap<LogRecordItem, number>()
let nextKey = 0
function keyOf(record: LogRecordItem): number {
  let key = keys.get(record)
  if (key === undefined) {
    key = nextKey++
    keys.set(record, key)
  }
  return key
}

const download = computed(() =>
  downloadUrl(
    queryOf({ ...filters.value, since: search.value.since, until: search.value.until }),
    apiLocation(),
  ),
)

const selected = ref<LogRecordItem>()
const detailOpen = computed({
  get: () => selected.value !== undefined,
  set: (open) => {
    if (!open) {
      selected.value = undefined
    }
  },
})

function sameRequest(requestId: string) {
  selected.value = undefined
  clear()
  draft.value.request_id = requestId
  moreOpen.value = true
}

const deletionsOpen = ref(false)
const deleteOpen = ref(false)
const deletion = ref({ site: '', since: '' })
const remove = useMutation(deleteLogsMutation())

function askToDelete() {
  deletion.value = { site: queried('site'), since: '' }
  deleteOpen.value = true
}

function confirmDelete() {
  remove.mutate(
    {
      body: { site: deletion.value.site || null, since: instant(deletion.value.since) ?? null },
      headers: plainHeaders(),
    },
    {
      onSuccess: () => {
        toast.success(t('logs.deleteRequested'))
        void queryClient.invalidateQueries({ queryKey: listLogDeletionsQueryKey() })
      },
      onError: (error) => notifyFailure(error, t('logs.deleteFailed')),
    },
  )
}
</script>

<template>
  <div class="@container flex flex-col gap-6">
    <PageHeader :icon="Logs" :title="t('logs.title')" :description="t('logs.description')">
      <template #actions>
        <Button v-if="following" variant="outline" size="sm" @click="tail.stop()">
          <Pause data-icon="inline-start" aria-hidden="true" />
          {{ t('logs.pause') }}
        </Button>
        <Button v-else variant="outline" size="sm" @click="follow">
          <Radio data-icon="inline-start" aria-hidden="true" />
          {{ t('logs.follow') }}
        </Button>
        <Button variant="outline" size="sm" as-child>
          <a :href="download" download>
            <Download data-icon="inline-start" aria-hidden="true" />
            {{ t('logs.download') }}
          </a>
        </Button>
        <Button variant="outline" size="sm" @click="deletionsOpen = true">
          <History data-icon="inline-start" aria-hidden="true" />
          {{ t('logs.deletions') }}
        </Button>
        <Button v-if="can('logs.delete')" variant="outline" size="sm" @click="askToDelete">
          <Trash2 data-icon="inline-start" aria-hidden="true" />
          {{ t('logs.delete') }}
        </Button>
      </template>
    </PageHeader>

    <form role="search" class="flex flex-col gap-3" @submit.prevent>
      <div
        class="grid gap-3 @xl:grid-cols-2 @4xl:grid-cols-[2fr_1fr_1fr_1fr_auto_auto] @4xl:items-end"
      >
        <div class="flex flex-col gap-1.5">
          <Label for="logs-text">{{ t('logs.text') }}</Label>
          <div class="relative">
            <Search
              class="text-muted-foreground pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2"
              aria-hidden="true"
            />
            <Input
              id="logs-text"
              v-model="draft.text"
              type="search"
              class="pl-8"
              autocomplete="off"
            />
          </div>
        </div>
        <div class="flex flex-col gap-1.5">
          <Label for="logs-kind">{{ t('logs.columns.kind') }}</Label>
          <Select v-model="kind">
            <SelectTrigger id="logs-kind" class="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem :value="ALL">{{ t('logs.allKinds') }}</SelectItem>
              <SelectItem value="access">{{ t('logs.kinds.access') }}</SelectItem>
              <SelectItem value="error">{{ t('logs.kinds.error') }}</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div class="flex flex-col gap-1.5">
          <Label for="logs-site">{{ t('logs.columns.site') }}</Label>
          <Input id="logs-site" v-model="draft.site" autocomplete="off" />
        </div>
        <div class="flex flex-col gap-1.5">
          <Label for="logs-status">{{ t('logs.columns.status') }}</Label>
          <Input
            id="logs-status"
            v-model="draft.status"
            :placeholder="t('logs.statusExample')"
            autocomplete="off"
          />
        </div>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          :aria-expanded="moreOpen"
          aria-controls="logs-more"
          @click="moreOpen = !moreOpen"
        >
          <ListFilter data-icon="inline-start" aria-hidden="true" />
          {{ t('logs.more') }}
        </Button>
        <Button type="button" variant="ghost" size="sm" :disabled="!filtered" @click="clear">
          <FilterX data-icon="inline-start" aria-hidden="true" />
          {{ t('logs.clear') }}
        </Button>
      </div>
      <div v-show="moreOpen" id="logs-more" class="grid gap-3 @xl:grid-cols-2 @4xl:grid-cols-3">
        <div class="flex flex-col gap-1.5">
          <Label for="logs-route">{{ t('logs.columns.route') }}</Label>
          <Input id="logs-route" v-model="draft.route" autocomplete="off" />
        </div>
        <div class="flex flex-col gap-1.5">
          <Label for="logs-client">{{ t('logs.columns.client') }}</Label>
          <Input
            id="logs-client"
            v-model="draft.client"
            class="font-mono"
            :placeholder="t('logs.clientExample')"
            autocomplete="off"
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <Label for="logs-path">{{ t('logs.pathPrefix') }}</Label>
          <Input
            id="logs-path"
            v-model="draft.path"
            class="font-mono"
            placeholder="/"
            autocomplete="off"
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <Label for="logs-request">{{ t('logs.columns.requestId') }}</Label>
          <Input
            id="logs-request"
            v-model="draft.request_id"
            class="font-mono"
            autocomplete="off"
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <Label for="logs-since">{{ t('logs.since') }}</Label>
          <Input id="logs-since" v-model="draft.since" type="datetime-local" step="1" />
        </div>
        <div class="flex flex-col gap-1.5">
          <Label for="logs-until">{{ t('logs.until') }}</Label>
          <Input id="logs-until" v-model="draft.until" type="datetime-local" step="1" />
        </div>
      </div>
    </form>

    <div v-if="following && !tailFailed" class="flex items-center gap-3">
      <StatusIndicator
        :tone="tail.state.value === 'live' ? 'positive' : 'pending'"
        :label="tail.state.value === 'live' ? t('logs.live') : t('logs.connecting')"
      />
      <span class="text-muted-foreground text-sm">{{
        t('logs.followed', { count: tail.items.value.length }, tail.items.value.length)
      }}</span>
    </div>
    <Alert v-if="tailFailed" variant="destructive">
      <Radio aria-hidden="true" />
      <AlertTitle>{{ t('logs.tailFailed') }}</AlertTitle>
      <AlertDescription class="flex flex-col items-start gap-2">
        <span>{{ tail.failure.value?.message ?? t('logs.tailLost') }}</span>
        <Button variant="outline" size="sm" @click="follow">
          <RotateCw data-icon="inline-start" aria-hidden="true" />
          {{ t('logs.followAgain') }}
        </Button>
      </AlertDescription>
    </Alert>

    <ApiFailureAlert
      v-if="pages.isError.value && !pages.data.value"
      :error="pages.error.value"
      retryable
      @retry="pages.refetch()"
    />
    <div v-else-if="pages.isPending.value" class="flex flex-col gap-2">
      <Skeleton v-for="index in 6" :key="index" class="h-11 w-full" />
    </div>
    <Empty v-else-if="rows.length === 0" class="border">
      <EmptyHeader>
        <EmptyMedia variant="icon"><Logs aria-hidden="true" /></EmptyMedia>
        <EmptyTitle>{{ filtered ? t('logs.noMatchTitle') : t('logs.emptyTitle') }}</EmptyTitle>
        <EmptyDescription>
          {{ filtered ? t('logs.noMatchDetail') : t('logs.emptyDetail') }}
        </EmptyDescription>
      </EmptyHeader>
    </Empty>

    <template v-else>
      <div class="rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>{{ t('logs.columns.time') }}</TableHead>
              <TableHead>{{ t('logs.columns.status') }}</TableHead>
              <TableHead>{{ t('logs.columns.request') }}</TableHead>
              <TableHead class="hidden md:table-cell">{{ t('logs.columns.site') }}</TableHead>
              <TableHead class="hidden lg:table-cell">{{ t('logs.columns.client') }}</TableHead>
              <TableHead class="hidden xl:table-cell">{{ t('logs.columns.requestId') }}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-for="record in rows" :key="keyOf(record)">
              <TableCell class="whitespace-nowrap">
                <button
                  type="button"
                  class="font-mono text-xs tabular-nums hover:underline"
                  :aria-label="t('logs.detail', { time: d(new Date(record.time), 'precise') })"
                  @click="selected = record"
                >
                  <time :datetime="record.time">{{ d(new Date(record.time), 'precise') }}</time>
                </button>
              </TableCell>
              <TableCell><LogStatus :record="record" /></TableCell>
              <TableCell class="max-w-96 truncate font-mono text-xs" :title="summaryOf(record)">
                {{ summaryOf(record) }}
              </TableCell>
              <TableCell class="hidden md:table-cell">{{ record.site ?? '—' }}</TableCell>
              <TableCell class="hidden font-mono text-xs lg:table-cell">
                {{ record.client ?? '—' }}
              </TableCell>
              <TableCell class="hidden xl:table-cell">
                <button
                  v-if="record.request_id"
                  type="button"
                  class="text-muted-foreground max-w-48 truncate font-mono text-xs hover:underline"
                  :title="record.request_id"
                  @click="sameRequest(record.request_id)"
                >
                  {{ record.request_id }}
                </button>
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>
      <div v-if="pages.hasNextPage.value" class="flex justify-center">
        <Button
          variant="outline"
          size="sm"
          :disabled="pages.isFetchingNextPage.value"
          @click="pages.fetchNextPage()"
        >
          <Spinner v-if="pages.isFetchingNextPage.value" data-icon="inline-start" />
          {{ t('common.loadMore') }}
        </Button>
      </div>
    </template>

    <LogRecordSheet v-model:open="detailOpen" :record="selected" @request="sameRequest" />
    <LogDeletionsSheet v-model:open="deletionsOpen" />
    <ConfirmDialog
      v-model:open="deleteOpen"
      :icon="Trash2"
      :title="t('logs.deleteTitle')"
      :description="t('logs.deleteDescription')"
      :confirm-label="t('logs.deleteConfirm')"
      :busy="remove.isPending.value"
      destructive
      @confirm="confirmDelete"
    >
      <div class="grid gap-3 sm:grid-cols-2">
        <div class="flex flex-col gap-1.5">
          <Label for="logs-delete-site">{{ t('logs.columns.site') }}</Label>
          <Input
            id="logs-delete-site"
            v-model="deletion.site"
            :placeholder="t('logs.allSites')"
            autocomplete="off"
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <Label for="logs-delete-since">{{ t('logs.deleteSince') }}</Label>
          <Input id="logs-delete-since" v-model="deletion.since" type="datetime-local" step="1" />
        </div>
      </div>
    </ConfirmDialog>
  </div>
</template>
