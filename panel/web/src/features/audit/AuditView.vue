<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useInfiniteQuery } from '@tanstack/vue-query'
import { watchDebounced } from '@vueuse/core'
import { FilterX, ScrollText, ShieldAlert, ShieldCheck, UserRound } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import {
  verifyAuditEvents,
  type AuditEvent,
  type AuditEventPage,
  type AuditVerificationResponse,
} from '@/api/generated'
import { listAuditEventsInfiniteOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
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
  SelectSeparator,
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
import { notifyFailure } from '@/lib/configuration'
import AuditEventSheet from './AuditEventSheet.vue'
import { isKnownType, KNOWN_TYPES, summaryOf, toneOf, typeKey } from './presentation'

const PAGE_SIZE = 50
const ALL = 'all'
const FIELDS = ['actor', 'type', 'correlation_id', 'since', 'until'] as const
type Field = (typeof FIELDS)[number]

const { t, d } = useI18n()
const route = useRoute()
const router = useRouter()

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

const type = computed({
  get: () => draft.value.type || ALL,
  set: (value: string) => (draft.value.type = value === ALL ? '' : value),
})

/** `datetime-local` values in the browser's zone, as RFC 3339. */
function instant(value: string): string | undefined {
  const time = value ? new Date(value) : undefined
  return time && !Number.isNaN(time.getTime()) ? time.toISOString() : undefined
}

const events = useInfiniteQuery(
  computed(() => ({
    ...listAuditEventsInfiniteOptions({
      query: {
        limit: PAGE_SIZE,
        actor: queried('actor') || undefined,
        type: queried('type') || undefined,
        correlation_id: queried('correlation_id') || undefined,
        since: instant(queried('since')),
        until: instant(queried('until')),
      },
    }),
    initialPageParam: {} as { query?: { before: number } },
    getNextPageParam: (page: AuditEventPage) =>
      page.next_before ? { query: { before: page.next_before } } : undefined,
  })),
)
const items = computed(() => events.data.value?.pages.flatMap((page) => page.items) ?? [])
const filtered = computed(() => FIELDS.some((field) => queried(field)))

function label(event: AuditEvent): string {
  return isKnownType(event.event_type) ? t(typeKey(event.event_type)) : event.event_type
}

function clear() {
  for (const field of FIELDS) {
    draft.value[field] = ''
  }
}

const selected = ref<AuditEvent>()
const detailOpen = computed({
  get: () => selected.value !== undefined,
  set: (open) => {
    if (!open) {
      selected.value = undefined
    }
  },
})

function correlate(correlation: string) {
  selected.value = undefined
  clear()
  draft.value.correlation_id = correlation
}

const verifying = ref(false)
const verification = ref<AuditVerificationResponse>()
async function verify() {
  verifying.value = true
  try {
    const { data } = await verifyAuditEvents({ throwOnError: true })
    verification.value = data
  } catch (error) {
    notifyFailure(error, t('audit.verifyFailed'))
  } finally {
    verifying.value = false
  }
}
</script>

<template>
  <div class="@container flex flex-col gap-6">
    <PageHeader :icon="ScrollText" :title="t('audit.title')" :description="t('audit.description')">
      <template #actions>
        <Button variant="outline" size="sm" :disabled="verifying" @click="verify">
          <Spinner v-if="verifying" data-icon="inline-start" />
          <ShieldCheck v-else data-icon="inline-start" aria-hidden="true" />
          {{ t('audit.verify') }}
        </Button>
      </template>
    </PageHeader>

    <Alert v-if="verification" role="status">
      <ShieldCheck v-if="verification.intact" aria-hidden="true" />
      <ShieldAlert v-else aria-hidden="true" />
      <AlertTitle>
        {{ verification.intact ? t('audit.intact') : t('audit.broken') }}
      </AlertTitle>
      <AlertDescription>
        {{
          verification.intact
            ? t(
                'audit.intactDetail',
                { checked: verification.checked, head: verification.head_sequence },
                verification.checked,
              )
            : t('audit.brokenDetail', { sequence: verification.first_mismatch ?? '' })
        }}
      </AlertDescription>
    </Alert>

    <div
      class="grid gap-3 @xl:grid-cols-2 @4xl:grid-cols-[1fr_1fr_1fr_auto_auto_auto] @4xl:items-end"
    >
      <div class="flex flex-col gap-1.5">
        <Label for="audit-actor">{{ t('audit.columns.actor') }}</Label>
        <Input id="audit-actor" v-model="draft.actor" autocomplete="off" />
      </div>
      <div class="flex flex-col gap-1.5">
        <Label for="audit-type">{{ t('audit.columns.type') }}</Label>
        <Select v-model="type">
          <SelectTrigger id="audit-type" class="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem :value="ALL">{{ t('audit.allTypes') }}</SelectItem>
            <SelectItem value="config.">{{ t('audit.configTypes') }}</SelectItem>
            <SelectItem value="gateway.">{{ t('audit.gatewayTypes') }}</SelectItem>
            <SelectSeparator />
            <SelectItem v-for="known in KNOWN_TYPES" :key="known" :value="known">
              {{ t(typeKey(known)) }}
            </SelectItem>
          </SelectContent>
        </Select>
      </div>
      <div class="flex flex-col gap-1.5">
        <Label for="audit-correlation">{{ t('audit.columns.correlation') }}</Label>
        <Input
          id="audit-correlation"
          v-model="draft.correlation_id"
          class="font-mono"
          autocomplete="off"
        />
      </div>
      <div class="flex flex-col gap-1.5">
        <Label for="audit-since">{{ t('audit.since') }}</Label>
        <Input id="audit-since" v-model="draft.since" type="datetime-local" />
      </div>
      <div class="flex flex-col gap-1.5">
        <Label for="audit-until">{{ t('audit.until') }}</Label>
        <Input id="audit-until" v-model="draft.until" type="datetime-local" />
      </div>
      <Button variant="ghost" size="sm" :disabled="!filtered" @click="clear">
        <FilterX data-icon="inline-start" aria-hidden="true" />
        {{ t('audit.clear') }}
      </Button>
    </div>

    <ApiFailureAlert
      v-if="events.isError.value && !events.data.value"
      :error="events.error.value"
      retryable
      @retry="events.refetch()"
    />
    <div v-else-if="events.isPending.value" class="flex flex-col gap-2">
      <Skeleton v-for="index in 5" :key="index" class="h-11 w-full" />
    </div>
    <Empty v-else-if="items.length === 0" class="border">
      <EmptyHeader>
        <EmptyMedia variant="icon"><ScrollText aria-hidden="true" /></EmptyMedia>
        <EmptyTitle>{{ filtered ? t('audit.noMatchTitle') : t('audit.emptyTitle') }}</EmptyTitle>
        <EmptyDescription>
          {{ filtered ? t('audit.noMatchDetail') : t('audit.emptyDetail') }}
        </EmptyDescription>
      </EmptyHeader>
    </Empty>

    <template v-else>
      <div class="rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead class="w-16">{{ t('audit.columns.sequence') }}</TableHead>
              <TableHead>{{ t('audit.columns.time') }}</TableHead>
              <TableHead>{{ t('audit.columns.actor') }}</TableHead>
              <TableHead>{{ t('audit.columns.event') }}</TableHead>
              <TableHead class="hidden md:table-cell">{{ t('audit.columns.summary') }}</TableHead>
              <TableHead class="hidden lg:table-cell">{{
                t('audit.columns.correlation')
              }}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-for="event in items" :key="event.sequence">
              <TableCell>
                <button
                  type="button"
                  class="font-mono font-medium tabular-nums hover:underline"
                  :aria-label="t('audit.detail', { sequence: event.sequence })"
                  @click="selected = event"
                >
                  #{{ event.sequence }}
                </button>
              </TableCell>
              <TableCell class="whitespace-nowrap">
                <time v-if="event.occurred_at" :datetime="event.occurred_at">{{
                  d(new Date(event.occurred_at), 'datetime')
                }}</time>
              </TableCell>
              <TableCell>
                <span class="flex items-center gap-1.5">
                  <UserRound class="size-3.5 shrink-0" aria-hidden="true" />
                  {{ event.actor_id || t('audit.unknownActor') }}
                </span>
              </TableCell>
              <TableCell>
                <StatusIndicator :tone="toneOf(event.event_type)" :label="label(event)" />
              </TableCell>
              <TableCell class="text-muted-foreground hidden max-w-96 truncate md:table-cell">
                {{ summaryOf(event, t) }}
              </TableCell>
              <TableCell class="hidden lg:table-cell">
                <button
                  type="button"
                  class="text-muted-foreground max-w-48 truncate font-mono text-xs hover:underline"
                  :title="event.correlation_id"
                  @click="correlate(event.correlation_id)"
                >
                  {{ event.correlation_id }}
                </button>
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>
      <div v-if="events.hasNextPage.value" class="flex justify-center">
        <Button
          variant="outline"
          size="sm"
          :disabled="events.isFetchingNextPage.value"
          @click="events.fetchNextPage()"
        >
          <Spinner v-if="events.isFetchingNextPage.value" data-icon="inline-start" />
          {{ t('common.loadMore') }}
        </Button>
      </div>
    </template>

    <AuditEventSheet v-model:open="detailOpen" :event="selected" @correlate="correlate" />
  </div>
</template>
