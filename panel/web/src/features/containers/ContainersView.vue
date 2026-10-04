<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { watchDebounced } from '@vueuse/core'
import { Boxes, Container, RefreshCw, Search } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { listContainersOptions, listEnginesOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { toApiFailure } from '@/lib/api'
import { useSession } from '@/lib/session'
import type { ContainerView } from '@/api/generated'
import ContainerActions from './ContainerActions.vue'
import ContainerDetailSheet from './ContainerDetailSheet.vue'
import ContainerEngines from './ContainerEngines.vue'
import {
  CONTAINER_STATES,
  chosenEngine,
  portLabel,
  REFRESH_INTERVAL_MS,
  withoutAgent,
} from './presentation'
import { engineName, stateTone } from '@/lib/containers'

const ALL = 'all'
const FIELDS = ['engine', 'search', 'state'] as const
type Field = (typeof FIELDS)[number]

const { t, d } = useI18n()
const { can } = useSession()
const route = useRoute()
const router = useRouter()

const engines = useQuery({
  ...listEnginesOptions(),
  refetchInterval: REFRESH_INTERVAL_MS,
  retry: (failures, error) => failures < 1 && !withoutAgent(toApiFailure(error)),
})
const engineList = computed(() => engines.data.value?.engines ?? [])
const missingAgent = computed(
  () => engines.isError.value && withoutAgent(toApiFailure(engines.error.value)),
)

function queried(field: Field): string {
  const value = route.query[field]
  return typeof value === 'string' ? value : ''
}

/** Filters as chosen; they reach the URL, and the query, after a pause. */
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

const engineId = computed(() => chosenEngine(engineList.value, queried('engine')))
const engine = computed(() => engineList.value.find((candidate) => candidate.id === engineId.value))
const selectedEngine = computed({
  get: () => engineId.value ?? '',
  set: (value: string) => (draft.value.engine = value),
})
const state = computed({
  get: () => draft.value.state || ALL,
  set: (value: string) => (draft.value.state = value === ALL ? '' : value),
})
const filtered = computed(() => Boolean(queried('search') || queried('state')))

const containers = useQuery(
  computed(() => ({
    ...listContainersOptions({
      path: { engine: engineId.value ?? '' },
      query: {
        search: queried('search') || undefined,
        state: queried('state') || undefined,
      },
    }),
    enabled: Boolean(engine.value?.enabled && engine.value.reachable),
    refetchInterval: REFRESH_INTERVAL_MS,
  })),
)
const rows = computed(() => containers.data.value?.containers ?? [])

const inspecting = ref<ContainerView>()
const detailOpen = ref(false)
function inspect(container: ContainerView) {
  inspecting.value = container
  detailOpen.value = true
}

const updatedAt = computed(() =>
  engines.dataUpdatedAt.value > 0 ? d(new Date(engines.dataUpdatedAt.value), 'time') : null,
)
const fetching = computed(() => engines.isFetching.value || containers.isFetching.value)

function refresh() {
  void engines.refetch()
  if (engine.value?.enabled && engine.value.reachable) {
    void containers.refetch()
  }
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="Container"
      :title="t('containers.title')"
      :description="t('containers.description')"
    >
      <template #actions>
        <span v-if="updatedAt" class="text-muted-foreground text-xs">
          {{ t('state.updatedAt', { time: updatedAt }) }}
        </span>
        <Button variant="outline" size="sm" :disabled="fetching" @click="refresh">
          <RefreshCw
            data-icon="inline-start"
            :class="{ 'animate-spin': fetching }"
            aria-hidden="true"
          />
          {{ t('state.refresh') }}
        </Button>
      </template>
    </PageHeader>

    <Empty v-if="missingAgent" class="border">
      <EmptyHeader>
        <EmptyMedia variant="icon"><Container aria-hidden="true" /></EmptyMedia>
        <EmptyTitle>{{ t('containers.noAgentTitle') }}</EmptyTitle>
        <EmptyDescription>{{ t('containers.noAgentDetail') }}</EmptyDescription>
      </EmptyHeader>
    </Empty>
    <ApiFailureAlert
      v-else-if="engines.isError.value && !engines.data.value"
      :error="engines.error.value"
      retryable
      @retry="engines.refetch()"
    />
    <div
      v-else-if="engines.isPending.value"
      class="grid gap-4 lg:grid-cols-2"
      aria-busy="true"
      :aria-label="t('state.loading')"
    >
      <Skeleton v-for="index in 2" :key="index" class="h-36 rounded-lg" />
    </div>
    <Empty v-else-if="engineList.length === 0" class="border">
      <EmptyHeader>
        <EmptyMedia variant="icon"><Container aria-hidden="true" /></EmptyMedia>
        <EmptyTitle>{{ t('containers.noEnginesTitle') }}</EmptyTitle>
        <EmptyDescription>{{ t('containers.noEnginesDetail') }}</EmptyDescription>
      </EmptyHeader>
    </Empty>

    <template v-else>
      <ContainerEngines :engines="engineList" />

      <Card class="min-w-0">
        <CardHeader>
          <CardTitle class="flex items-center gap-2">
            <Boxes class="size-4" aria-hidden="true" />{{ t('containers.list.title') }}
          </CardTitle>
        </CardHeader>
        <CardContent class="flex flex-col gap-4">
          <div class="flex flex-wrap items-center gap-2">
            <Select v-if="engineList.length > 1" v-model="selectedEngine">
              <SelectTrigger class="w-36" :aria-label="t('containers.list.engine')">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem v-for="option in engineList" :key="option.id" :value="option.id">
                  {{ engineName(option.id) }}
                </SelectItem>
              </SelectContent>
            </Select>
            <div class="relative min-w-56 flex-1">
              <Search
                class="text-muted-foreground absolute top-1/2 left-2.5 size-4 -translate-y-1/2"
                aria-hidden="true"
              />
              <Input
                v-model="draft.search"
                type="search"
                class="pl-8"
                :placeholder="t('containers.list.searchPlaceholder')"
                :aria-label="t('common.search')"
              />
            </div>
            <Select v-model="state">
              <SelectTrigger class="w-40" :aria-label="t('containers.list.state')">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem :value="ALL">{{ t('containers.list.allStates') }}</SelectItem>
                <SelectItem v-for="option in CONTAINER_STATES" :key="option" :value="option">
                  {{ t(`containers.states.${option}`) }}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>

          <p v-if="engine && !engine.enabled" class="text-muted-foreground text-sm">
            {{ t('containers.list.engineDisabled', { engine: engineName(engine.id) }) }}
          </p>
          <p v-else-if="engine && !engine.reachable" class="text-muted-foreground text-sm">
            {{ t('containers.list.engineUnreachable', { engine: engineName(engine.id) }) }}
          </p>
          <ApiFailureAlert
            v-else-if="containers.isError.value && !containers.data.value"
            :error="containers.error.value"
            retryable
            @retry="containers.refetch()"
          />
          <Skeleton
            v-else-if="containers.isPending.value"
            class="h-24 rounded-lg"
            aria-busy="true"
            :aria-label="t('state.loading')"
          />
          <div v-else-if="rows.length" class="overflow-x-auto">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{{ t('containers.list.name') }}</TableHead>
                  <TableHead>{{ t('containers.list.image') }}</TableHead>
                  <TableHead>{{ t('containers.list.state') }}</TableHead>
                  <TableHead>{{ t('containers.list.ports') }}</TableHead>
                  <TableHead>{{ t('containers.list.created') }}</TableHead>
                  <TableHead v-if="can('containers.manage')">
                    <span class="sr-only">{{ t('common.actions') }}</span>
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                <TableRow v-for="container in rows" :key="container.id">
                  <TableCell>
                    <div class="flex flex-col gap-1">
                      <Button
                        v-if="can('containers.inspect')"
                        variant="link"
                        class="h-auto justify-start p-0 text-left font-medium break-all whitespace-normal"
                        @click="inspect(container)"
                      >
                        {{ container.names[0] ?? container.id }}
                      </Button>
                      <span v-else class="font-medium break-all">{{
                        container.names[0] ?? container.id
                      }}</span>
                      <span class="flex flex-wrap items-center gap-1">
                        <span class="text-muted-foreground font-mono text-xs">
                          {{ container.id.slice(0, 12) }}
                        </span>
                        <Badge v-if="container.compose_project" variant="secondary">
                          {{ t('containers.list.project', { project: container.compose_project }) }}
                        </Badge>
                      </span>
                    </div>
                  </TableCell>
                  <TableCell class="font-mono text-xs break-all">{{ container.image }}</TableCell>
                  <TableCell>
                    <div class="flex flex-col gap-1">
                      <StatusIndicator
                        :tone="stateTone(container.state)"
                        :label="t(`containers.states.${container.state}`)"
                      />
                      <span class="text-muted-foreground text-xs">{{ container.status }}</span>
                    </div>
                  </TableCell>
                  <TableCell>
                    <ul v-if="container.ports.length" class="flex flex-col gap-0.5">
                      <li
                        v-for="port in container.ports"
                        :key="portLabel(port)"
                        class="font-mono text-xs whitespace-nowrap"
                      >
                        {{ portLabel(port) }}
                      </li>
                    </ul>
                    <span v-else class="text-muted-foreground text-sm">—</span>
                  </TableCell>
                  <TableCell class="text-muted-foreground text-sm whitespace-nowrap">
                    {{ container.created ? d(new Date(container.created), 'datetime') : '—' }}
                  </TableCell>
                  <TableCell v-if="can('containers.manage') && engineId" class="text-right">
                    <ContainerActions :engine="engineId" :container="container" />
                  </TableCell>
                </TableRow>
              </TableBody>
            </Table>
          </div>
          <Empty v-else class="border border-dashed">
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <Search v-if="filtered" aria-hidden="true" />
                <Boxes v-else aria-hidden="true" />
              </EmptyMedia>
              <EmptyTitle>
                {{ filtered ? t('containers.list.noMatch') : t('containers.list.empty') }}
              </EmptyTitle>
            </EmptyHeader>
          </Empty>
        </CardContent>
      </Card>
      <ContainerDetailSheet
        v-if="inspecting && engineId"
        v-model:open="detailOpen"
        :engine="engineId"
        :container="inspecting"
      />
    </template>
  </div>
</template>
