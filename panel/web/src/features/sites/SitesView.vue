<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useInfiniteQuery, useQuery } from '@tanstack/vue-query'
import { refDebounced } from '@vueuse/core'
import {
  ArchiveRestore,
  ArrowDownWideNarrow,
  ArrowUpNarrowWide,
  CircleCheck,
  CirclePause,
  Copy,
  Download,
  Ellipsis,
  Globe,
  Lock,
  Pause,
  Pencil,
  Play,
  Plus,
  Search,
  ShieldCheck,
  Star,
  Trash2,
  TriangleAlert,
  Upload,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type {
  BatchAction,
  SiteKind,
  SiteList,
  SiteSort,
  SiteStatus,
  SiteView,
} from '@/api/generated'
import {
  listSitesInfiniteOptions,
  siteSummaryOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatTile from '@/components/StatTile.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from '@/components/ui/empty'
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
import { SITE_KINDS } from './forms'
import SiteFormSheet from './SiteFormSheet.vue'
import { useSiteCommands } from './useSiteCommands'
import { kindIcons, statusTones } from './presentation'

const ALL = 'all'
const STATUSES: readonly SiteStatus[] = ['running', 'stopped', 'abnormal']
const SORTS: readonly SiteSort[] = ['name', 'updated_at', 'created_at', 'status', 'domain']

const { t, d } = useI18n()
const commands = useSiteCommands()

const search = ref('')
const status = ref<SiteStatus | typeof ALL>(ALL)
const kind = ref<SiteKind | typeof ALL>(ALL)
const domain = ref('')
const tag = ref('')
const group = ref('')
const favorites = ref(false)
const sort = ref<SiteSort>('name')
const descending = ref(false)
const debounced = refDebounced(
  computed(() => [search.value, domain.value, tag.value, group.value] as const),
  250,
)
const recycleBin = computed(() => status.value === 'deleted')

const query = computed(() => {
  const [q, byDomain, byTag, byGroup] = debounced.value
  return {
    q: q.trim() || undefined,
    status: status.value === ALL ? undefined : status.value,
    kind: kind.value === ALL ? undefined : kind.value,
    domain: byDomain.trim() || undefined,
    tag: byTag.trim() || undefined,
    group: byGroup.trim() || undefined,
    favorite: favorites.value ? true : undefined,
    sort: sort.value,
    descending: descending.value,
  }
})

const summary = useQuery(siteSummaryOptions())
const sites = useInfiniteQuery(
  computed(() => ({
    ...listSitesInfiniteOptions({ query: query.value }),
    initialPageParam: {},
    getNextPageParam: (page: SiteList) => page.next_cursor ?? undefined,
  })),
)
const rows = computed(() => sites.data.value?.pages.flatMap((page) => page.items) ?? [])
const total = computed(() => sites.data.value?.pages[0]?.total ?? 0)
const filtered = computed(
  () =>
    Boolean(debounced.value.some((value) => value.trim() !== '')) ||
    status.value !== ALL ||
    kind.value !== ALL ||
    favorites.value,
)
const updatedAt = computed(() =>
  summary.dataUpdatedAt.value > 0 ? d(new Date(summary.dataUpdatedAt.value), 'time') : null,
)

const selected = ref<string[]>([])
watch(query, () => (selected.value = []))
const allSelected = computed(
  () => rows.value.length > 0 && rows.value.every((site) => selected.value.includes(site.id)),
)
function selectAll(checked: boolean | 'indeterminate') {
  selected.value = checked === true ? rows.value.map((site) => site.id) : []
}
function select(id: string, checked: boolean | 'indeterminate') {
  selected.value =
    checked === true ? [...selected.value, id] : selected.value.filter((value) => value !== id)
}

const tiles = computed(
  () =>
    [
      { id: ALL, icon: Globe, label: t('sites.summary.total'), value: summary.data.value?.total },
      {
        id: 'running',
        icon: CircleCheck,
        label: t('sites.summary.running'),
        value: summary.data.value?.running,
      },
      {
        id: 'stopped',
        icon: CirclePause,
        label: t('sites.summary.stopped'),
        value: summary.data.value?.stopped,
      },
      {
        id: 'abnormal',
        icon: TriangleAlert,
        label: t('sites.summary.abnormal'),
        value: summary.data.value?.abnormal,
      },
      {
        id: 'deleted',
        icon: Trash2,
        label: t('sites.summary.deleted'),
        value: summary.data.value?.deleted,
      },
    ] as const,
)
const kindTiles = computed(() =>
  SITE_KINDS.map((value) => ({
    id: value,
    icon: kindIcons[value],
    label: t(`sites.kind.${value}`),
    value: summary.data.value?.[value],
  })),
)

const editing = ref<SiteView | undefined>()
const formOpen = ref(false)
function openForm(site?: SiteView) {
  editing.value = site
  formOpen.value = true
}

const confirming = ref<{ action: 'delete' | 'purge'; sites: SiteView[] } | null>(null)
const confirmOpen = computed({
  get: () => confirming.value !== null,
  set: (open) => {
    if (!open) {
      confirming.value = null
    }
  },
})
function confirm() {
  const pending = confirming.value
  if (!pending) {
    return
  }
  const [only] = pending.sites
  if (pending.sites.length === 1 && only) {
    commands.remove(only, pending.action === 'purge', () => (confirming.value = null))
  } else {
    runBatch(pending.action, () => (confirming.value = null))
  }
}
function runBatch(action: BatchAction, onDone?: () => void) {
  commands.batch(action, [...selected.value], () => {
    selected.value = []
    onDone?.()
  })
}
function selectedSites() {
  return rows.value.filter((site) => selected.value.includes(site.id))
}

const fileInput = ref<HTMLInputElement | null>(null)
async function importSelected(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (file) {
    await commands.importFile(file)
  }
}

function primaryHost(site: SiteView) {
  const domains = site.domains ?? []
  const primary = domains.find((domain) => domain.primary) ?? domains[0]
  return primary ? (site.unicode_hosts?.[primary.host] ?? primary.host) : null
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader :icon="Globe" :title="t('sites.title')" :description="t('sites.description')">
      <template #actions>
        <span v-if="updatedAt" class="text-muted-foreground text-xs">
          {{ t('state.updatedAt', { time: updatedAt }) }}
        </span>
        <input
          ref="fileInput"
          type="file"
          accept="application/json,.json"
          class="hidden"
          @change="importSelected"
        />
        <Button
          variant="outline"
          size="sm"
          :disabled="commands.busy.value"
          @click="fileInput?.click()"
        >
          <Upload data-icon="inline-start" aria-hidden="true" />
          {{ t('common.import') }}
        </Button>
        <Button variant="outline" size="sm" @click="commands.exportSites(selected)">
          <Download data-icon="inline-start" aria-hidden="true" />
          {{ t('common.export') }}
        </Button>
        <Button size="sm" @click="openForm()">
          <Plus data-icon="inline-start" aria-hidden="true" />
          {{ t('sites.new') }}
        </Button>
      </template>
    </PageHeader>

    <section
      class="grid gap-3 sm:grid-cols-3 lg:grid-cols-5"
      :aria-label="t('sites.summary.total')"
    >
      <StatTile
        v-for="tile in tiles"
        :key="tile.id"
        :icon="tile.icon"
        :label="tile.label"
        :value="tile.value"
        :active="status === tile.id"
        @select="status = tile.id"
      />
    </section>
    <section class="grid gap-3 sm:grid-cols-2 lg:grid-cols-5">
      <StatTile
        v-for="tile in kindTiles"
        :key="tile.id"
        :icon="tile.icon"
        :label="tile.label"
        :value="tile.value"
        :active="kind === tile.id"
        @select="kind = kind === tile.id ? ALL : tile.id"
      />
      <StatTile
        :icon="Lock"
        :label="t('sites.summary.https')"
        :value="summary.data.value?.https"
        passive
      />
    </section>

    <div class="flex flex-col gap-3">
      <div class="flex flex-wrap items-center gap-2">
        <div class="relative min-w-56 flex-1">
          <Search
            class="text-muted-foreground absolute top-1/2 left-2.5 size-4 -translate-y-1/2"
            aria-hidden="true"
          />
          <Input
            v-model="search"
            type="search"
            class="pl-8"
            :placeholder="t('sites.searchPlaceholder')"
            :aria-label="t('common.search')"
          />
        </div>
        <Select v-model="status">
          <SelectTrigger class="w-36" :aria-label="t('sites.columns.status')">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem :value="ALL">{{ t('sites.allStatuses') }}</SelectItem>
            <SelectItem v-for="value in STATUSES" :key="value" :value="value">
              {{ t(`sites.status.${value}`) }}
            </SelectItem>
            <SelectItem value="deleted">{{ t('sites.recycleBin') }}</SelectItem>
          </SelectContent>
        </Select>
        <Select v-model="kind">
          <SelectTrigger class="w-36" :aria-label="t('sites.columns.kind')">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem :value="ALL">{{ t('sites.allKinds') }}</SelectItem>
            <SelectItem v-for="value in SITE_KINDS" :key="value" :value="value">
              {{ t(`sites.kind.${value}`) }}
            </SelectItem>
          </SelectContent>
        </Select>
        <Button
          variant="outline"
          size="sm"
          :aria-pressed="favorites"
          :class="{ 'bg-accent': favorites }"
          @click="favorites = !favorites"
        >
          <Star
            data-icon="inline-start"
            :class="{ 'fill-current': favorites }"
            aria-hidden="true"
          />
          {{ t('sites.favoritesOnly') }}
        </Button>
      </div>
      <div class="flex flex-wrap items-center gap-2">
        <Input
          v-model="domain"
          class="w-48"
          :placeholder="t('sites.columns.domains')"
          :aria-label="t('sites.columns.domains')"
        />
        <Input
          v-model="tag"
          class="w-36"
          :placeholder="t('sites.form.tags')"
          :aria-label="t('sites.form.tags')"
        />
        <Input
          v-model="group"
          class="w-36"
          :placeholder="t('sites.form.group')"
          :aria-label="t('sites.form.group')"
        />
        <div class="ml-auto flex items-center gap-2">
          <Select v-model="sort">
            <SelectTrigger class="w-36" :aria-label="t('sites.sortBy')">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="value in SORTS" :key="value" :value="value">
                {{ t(`sites.sort.${value}`) }}
              </SelectItem>
            </SelectContent>
          </Select>
          <Button
            variant="outline"
            size="icon-sm"
            :aria-pressed="descending"
            :aria-label="t('sites.descending')"
            @click="descending = !descending"
          >
            <ArrowDownWideNarrow v-if="descending" aria-hidden="true" />
            <ArrowUpNarrowWide v-else aria-hidden="true" />
          </Button>
        </div>
      </div>
    </div>

    <div
      v-if="selected.length > 0"
      class="bg-muted/50 flex flex-wrap items-center gap-2 rounded-lg border px-3 py-2"
      role="toolbar"
      :aria-label="t('common.actions')"
    >
      <span class="text-sm font-medium">{{ t('sites.selected', { count: selected.length }) }}</span>
      <div class="ml-auto flex flex-wrap gap-2">
        <template v-if="recycleBin">
          <Button
            variant="outline"
            size="sm"
            :disabled="commands.busy.value"
            @click="runBatch('restore')"
          >
            <ArchiveRestore data-icon="inline-start" aria-hidden="true" />
            {{ t('sites.batch.restore') }}
          </Button>
          <Button
            variant="destructive"
            size="sm"
            @click="confirming = { action: 'purge', sites: selectedSites() }"
          >
            <Trash2 data-icon="inline-start" aria-hidden="true" />
            {{ t('sites.batch.purge') }}
          </Button>
        </template>
        <template v-else>
          <Button
            variant="outline"
            size="sm"
            :disabled="commands.busy.value"
            @click="runBatch('enable')"
          >
            <Play data-icon="inline-start" aria-hidden="true" />
            {{ t('sites.batch.enable') }}
          </Button>
          <Button
            variant="outline"
            size="sm"
            :disabled="commands.busy.value"
            @click="runBatch('disable')"
          >
            <Pause data-icon="inline-start" aria-hidden="true" />
            {{ t('sites.batch.disable') }}
          </Button>
          <Button variant="outline" size="sm" @click="commands.validate(selected)">
            <ShieldCheck data-icon="inline-start" aria-hidden="true" />
            {{ t('sites.batch.validate') }}
          </Button>
          <Button
            variant="destructive"
            size="sm"
            @click="confirming = { action: 'delete', sites: selectedSites() }"
          >
            <Trash2 data-icon="inline-start" aria-hidden="true" />
            {{ t('sites.batch.delete') }}
          </Button>
        </template>
      </div>
    </div>

    <ApiFailureAlert
      v-if="sites.isError.value && !sites.data.value"
      :error="sites.error.value"
      retryable
      @retry="sites.refetch()"
    />

    <div v-else-if="sites.isPending.value" class="flex flex-col gap-2">
      <Skeleton v-for="index in 5" :key="index" class="h-12 w-full" />
    </div>

    <Empty v-else-if="rows.length === 0" class="border">
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <Search v-if="filtered" aria-hidden="true" />
          <Globe v-else aria-hidden="true" />
        </EmptyMedia>
        <EmptyTitle>{{ filtered ? t('sites.noMatchTitle') : t('sites.emptyTitle') }}</EmptyTitle>
        <EmptyDescription>
          {{ filtered ? t('sites.noMatchDetail') : t('sites.emptyDetail') }}
        </EmptyDescription>
      </EmptyHeader>
      <EmptyContent v-if="!filtered">
        <Button size="sm" @click="openForm()">
          <Plus data-icon="inline-start" aria-hidden="true" />
          {{ t('sites.new') }}
        </Button>
      </EmptyContent>
    </Empty>

    <div v-else class="rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead class="w-10">
              <Checkbox
                :model-value="allSelected"
                :aria-label="t('common.all')"
                @update:model-value="selectAll"
              />
            </TableHead>
            <TableHead class="w-10"
              ><span class="sr-only">{{ t('sites.favorite') }}</span></TableHead
            >
            <TableHead>{{ t('sites.columns.name') }}</TableHead>
            <TableHead>{{ t('sites.columns.status') }}</TableHead>
            <TableHead>{{ t('sites.columns.kind') }}</TableHead>
            <TableHead>{{ t('sites.columns.domains') }}</TableHead>
            <TableHead>{{ t('sites.columns.tags') }}</TableHead>
            <TableHead>{{ t('sites.columns.updated') }}</TableHead>
            <TableHead class="w-12"
              ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow
            v-for="site in rows"
            :key="site.id"
            :data-state="selected.includes(site.id) ? 'selected' : undefined"
          >
            <TableCell>
              <Checkbox
                :model-value="selected.includes(site.id)"
                :aria-label="site.name"
                @update:model-value="select(site.id, $event)"
              />
            </TableCell>
            <TableCell>
              <Button
                variant="ghost"
                size="icon-xs"
                :aria-pressed="site.favorite ?? false"
                :aria-label="site.favorite ? t('sites.unfavorite') : t('sites.favorite')"
                :disabled="recycleBin"
                @click="commands.setFavorite(site, !site.favorite)"
              >
                <Star :class="{ 'fill-current': site.favorite }" aria-hidden="true" />
              </Button>
            </TableCell>
            <TableCell class="max-w-64">
              <RouterLink
                :to="`/sites/${site.id}`"
                class="block truncate font-medium hover:underline"
              >
                {{ site.name }}
              </RouterLink>
              <span v-if="site.group" class="text-muted-foreground text-xs">{{ site.group }}</span>
            </TableCell>
            <TableCell>
              <StatusIndicator
                :tone="statusTones[site.status]"
                :label="t(`sites.status.${site.status}`)"
              />
            </TableCell>
            <TableCell>
              <span class="inline-flex items-center gap-1.5 text-sm">
                <component :is="kindIcons[site.kind]" class="size-4" aria-hidden="true" />
                {{ t(`sites.kind.${site.kind}`) }}
              </span>
            </TableCell>
            <TableCell class="max-w-64">
              <span
                v-if="primaryHost(site)"
                class="inline-flex items-center gap-1.5 font-mono text-xs"
              >
                <Lock v-if="site.https" class="size-3.5 shrink-0" :aria-label="t('sites.https')" />
                <span class="truncate">{{ primaryHost(site) }}</span>
                <Badge v-if="(site.domains?.length ?? 0) > 1" variant="secondary">
                  +{{ (site.domains?.length ?? 0) - 1 }}
                </Badge>
              </span>
              <span v-else class="text-muted-foreground text-xs">{{ t('sites.noDomains') }}</span>
            </TableCell>
            <TableCell>
              <div class="flex flex-wrap gap-1">
                <Badge v-for="item in site.tags ?? []" :key="item" variant="outline">{{
                  item
                }}</Badge>
              </div>
            </TableCell>
            <TableCell class="text-muted-foreground text-xs tabular-nums">
              {{ d(new Date(site.updated_at), 'datetime') }}
            </TableCell>
            <TableCell>
              <DropdownMenu>
                <DropdownMenuTrigger as-child>
                  <Button variant="ghost" size="icon-sm" :aria-label="t('common.actions')">
                    <Ellipsis aria-hidden="true" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <template v-if="site.status === 'deleted'">
                    <DropdownMenuItem @select="commands.restore(site)">
                      <ArchiveRestore aria-hidden="true" />
                      {{ t('sites.restore') }}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      variant="destructive"
                      @select="confirming = { action: 'purge', sites: [site] }"
                    >
                      <Trash2 aria-hidden="true" />
                      {{ t('sites.purge') }}
                    </DropdownMenuItem>
                  </template>
                  <template v-else>
                    <DropdownMenuItem @select="openForm(site)">
                      <Pencil aria-hidden="true" />
                      {{ t('common.edit') }}
                    </DropdownMenuItem>
                    <DropdownMenuItem @select="commands.setEnabled(site, !(site.enabled ?? true))">
                      <component :is="(site.enabled ?? true) ? Pause : Play" aria-hidden="true" />
                      {{ (site.enabled ?? true) ? t('common.disable') : t('common.enable') }}
                    </DropdownMenuItem>
                    <DropdownMenuItem @select="commands.clone(site)">
                      <Copy aria-hidden="true" />
                      {{ t('sites.clone') }}
                    </DropdownMenuItem>
                    <DropdownMenuItem @select="commands.exportSites([site.id])">
                      <Download aria-hidden="true" />
                      {{ t('common.export') }}
                    </DropdownMenuItem>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem
                      variant="destructive"
                      @select="confirming = { action: 'delete', sites: [site] }"
                    >
                      <Trash2 aria-hidden="true" />
                      {{ t('common.delete') }}
                    </DropdownMenuItem>
                  </template>
                </DropdownMenuContent>
              </DropdownMenu>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
      <div
        class="text-muted-foreground flex items-center justify-between border-t px-4 py-2 text-xs"
      >
        <span>{{ rows.length }} / {{ total }}</span>
        <Button
          v-if="sites.hasNextPage.value"
          variant="outline"
          size="sm"
          :disabled="sites.isFetchingNextPage.value"
          @click="sites.fetchNextPage()"
        >
          {{ t('common.loadMore') }}
        </Button>
      </div>
    </div>

    <SiteFormSheet v-model:open="formOpen" :site="editing" />

    <ConfirmDialog
      v-model:open="confirmOpen"
      :icon="Trash2"
      :title="
        confirming?.action === 'purge'
          ? t('sites.confirmPurgeTitle', { count: confirming?.sites.length ?? 0 })
          : t('sites.confirmDeleteTitle', { count: confirming?.sites.length ?? 0 })
      "
      :description="
        confirming?.action === 'purge'
          ? t('sites.confirmPurgeDetail')
          : t('sites.confirmDeleteDetail')
      "
      :confirm-label="confirming?.action === 'purge' ? t('sites.purge') : t('common.delete')"
      destructive
      :busy="commands.busy.value"
      @confirm="confirm"
    >
      <ul class="text-muted-foreground max-h-40 list-inside list-disc overflow-y-auto text-sm">
        <li v-for="site in confirming?.sites ?? []" :key="site.id">{{ site.name }}</li>
      </ul>
    </ConfirmDialog>
  </div>
</template>
