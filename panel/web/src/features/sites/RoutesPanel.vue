<script setup lang="ts">
import { computed, nextTick, ref, useTemplateRef, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { moveArrayElement, useSortable } from '@vueuse/integrations/useSortable'
import {
  ChevronDown,
  ChevronUp,
  GripVertical,
  Pencil,
  Plus,
  Route as RouteIcon,
  Trash2,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { RouteView, SiteView } from '@/api/generated'
import {
  deleteRouteMutation,
  listRoutesOptions,
  listUpstreamsOptions,
  reorderRoutesMutation,
  replaceRouteMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from '@/components/ui/empty'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { Table, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import {
  changeHeaders,
  notifyFailure,
  plainHeaders,
  useRefreshConfiguration,
} from '@/lib/configuration'
import { nextPriority, routeInputOf } from './forms'
import RouteFormSheet from './RouteFormSheet.vue'
import { actionIcons } from './presentation'

const props = defineProps<{ site: SiteView }>()

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const routes = useQuery(computed(() => listRoutesOptions({ path: { id: props.site.id } })))
const upstreams = useQuery(listUpstreamsOptions())
const reorder = useMutation(reorderRoutesMutation())
const replace = useMutation(replaceRouteMutation())
const remove = useMutation(deleteRouteMutation())

const order = ref<RouteView[]>([])
watch(
  () => routes.data.value,
  (value) => (order.value = [...(value ?? [])]),
  { immediate: true },
)

const onError = (error: unknown) => notifyFailure(error, t('common.changeFailed'))

function save() {
  reorder.mutate(
    {
      path: { id: props.site.id },
      body: { order: order.value.map((route) => route.id) },
      headers: plainHeaders(),
    },
    {
      onSuccess: (saved) => {
        order.value = saved
        toast.success(t('routes.reordered'))
        void refresh()
      },
      onError: (error) => {
        order.value = [...(routes.data.value ?? [])]
        onError(error)
      },
    },
  )
}

const body = useTemplateRef<HTMLElement>('body')
useSortable(body, order, {
  handle: '[data-drag-handle]',
  animation: 150,
  watchElement: true,
  onUpdate: (event) => {
    if (event.oldIndex === undefined || event.newIndex === undefined) {
      return
    }
    moveArrayElement(order, event.oldIndex, event.newIndex, event)
    void nextTick(save)
  },
})

function move(index: number, step: number) {
  const target = index + step
  if (target < 0 || target >= order.value.length) {
    return
  }
  const next = [...order.value]
  const [route] = next.splice(index, 1)
  next.splice(target, 0, route!)
  order.value = next
  save()
}

function setEnabled(route: RouteView, enabled: boolean) {
  replace.mutate(
    {
      path: { id: route.id },
      body: { ...routeInputOf(route), enabled },
      headers: changeHeaders(route.etag),
    },
    { onSuccess: () => void refresh(), onError },
  )
}

const editing = ref<RouteView | undefined>()
const formOpen = ref(false)
function openForm(route?: RouteView) {
  editing.value = route
  formOpen.value = true
}

const removing = ref<RouteView | null>(null)
const removeOpen = computed({
  get: () => removing.value !== null,
  set: (open) => {
    if (!open) {
      removing.value = null
    }
  },
})
function confirmRemove() {
  const route = removing.value
  if (!route) {
    return
  }
  remove.mutate(
    { path: { id: route.id }, headers: changeHeaders(route.etag) },
    {
      onSuccess: () => {
        toast.success(t('routes.deleted'))
        removing.value = null
        void refresh()
      },
      onError,
    },
  )
}

const upstreamNames = computed(
  () => new Map((upstreams.data.value ?? []).map((upstream) => [upstream.id, upstream.name])),
)
function target(route: RouteView): string {
  const action = route.action
  switch (action.type) {
    case 'proxy':
      return upstreamNames.value.get(action.upstream_id) ?? action.upstream_id
    case 'static':
      return action.root
    case 'redirect':
      return `${action.status ?? 308} → ${action.location}`
    case 'respond':
      return String(action.status ?? 503)
  }
}
</script>

<template>
  <div class="flex flex-col gap-4">
    <div class="flex flex-wrap items-center justify-between gap-2">
      <p class="text-muted-foreground text-sm">{{ t('routes.order') }}</p>
      <Button size="sm" @click="openForm()">
        <Plus data-icon="inline-start" aria-hidden="true" />
        {{ t('routes.add') }}
      </Button>
    </div>

    <ApiFailureAlert
      v-if="routes.isError.value"
      :error="routes.error.value"
      retryable
      @retry="routes.refetch()"
    />
    <Skeleton v-else-if="routes.isPending.value" class="h-32 w-full" />

    <Empty v-else-if="order.length === 0" class="border">
      <EmptyHeader>
        <EmptyMedia variant="icon"><RouteIcon aria-hidden="true" /></EmptyMedia>
        <EmptyTitle>{{ t('routes.emptyTitle') }}</EmptyTitle>
        <EmptyDescription>{{ t('routes.emptyDetail') }}</EmptyDescription>
      </EmptyHeader>
      <EmptyContent>
        <Button size="sm" @click="openForm()">
          <Plus data-icon="inline-start" aria-hidden="true" />
          {{ t('routes.add') }}
        </Button>
      </EmptyContent>
    </Empty>

    <div v-else class="rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead class="w-24"
              ><span class="sr-only">{{ t('routes.priority') }}</span></TableHead
            >
            <TableHead>{{ t('routes.name') }}</TableHead>
            <TableHead>{{ t('routes.match') }}</TableHead>
            <TableHead>{{ t('routes.action') }}</TableHead>
            <TableHead>{{ t('routes.priority') }}</TableHead>
            <TableHead>{{ t('common.enabled') }}</TableHead>
            <TableHead class="w-20"
              ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <tbody ref="body" data-slot="table-body" class="[&_tr:last-child]:border-0">
          <TableRow v-for="(route, index) in order" :key="route.id">
            <TableCell>
              <div class="flex items-center gap-0.5">
                <span
                  data-drag-handle
                  class="text-muted-foreground flex size-7 cursor-grab items-center justify-center active:cursor-grabbing"
                  aria-hidden="true"
                >
                  <GripVertical class="size-4" />
                </span>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  :disabled="index === 0 || reorder.isPending.value"
                  :aria-label="t('routes.moveUp', { name: route.name ?? route.match.path })"
                  @click="move(index, -1)"
                >
                  <ChevronUp aria-hidden="true" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  :disabled="index === order.length - 1 || reorder.isPending.value"
                  :aria-label="t('routes.moveDown', { name: route.name ?? route.match.path })"
                  @click="move(index, 1)"
                >
                  <ChevronDown aria-hidden="true" />
                </Button>
              </div>
            </TableCell>
            <TableCell class="font-medium">{{ route.name ?? '—' }}</TableCell>
            <TableCell class="max-w-72">
              <div class="flex items-center gap-2">
                <Badge variant="outline">{{ t(`routes.kinds.${route.match.kind}`) }}</Badge>
                <span class="truncate font-mono text-xs">{{ route.match.path }}</span>
              </div>
              <span v-if="route.match.host" class="text-muted-foreground font-mono text-xs">
                {{ route.match.host }}
              </span>
            </TableCell>
            <TableCell class="max-w-64">
              <span class="inline-flex items-center gap-1.5 text-sm">
                <component
                  :is="actionIcons[route.action.type]"
                  class="size-4 shrink-0"
                  aria-hidden="true"
                />
                {{ t(`routes.actions.${route.action.type}`) }}
              </span>
              <span class="text-muted-foreground block truncate font-mono text-xs">{{
                target(route)
              }}</span>
            </TableCell>
            <TableCell class="tabular-nums">{{ route.priority }}</TableCell>
            <TableCell>
              <Switch
                :model-value="route.enabled ?? true"
                :disabled="replace.isPending.value"
                :aria-label="t('common.enabled')"
                @update:model-value="setEnabled(route, $event)"
              />
            </TableCell>
            <TableCell>
              <div class="flex gap-1">
                <Button
                  variant="ghost"
                  size="icon-sm"
                  :aria-label="t('common.edit')"
                  @click="openForm(route)"
                >
                  <Pencil aria-hidden="true" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  :aria-label="t('common.delete')"
                  @click="removing = route"
                >
                  <Trash2 aria-hidden="true" />
                </Button>
              </div>
            </TableCell>
          </TableRow>
        </tbody>
      </Table>
    </div>

    <RouteFormSheet
      v-model:open="formOpen"
      :site="site"
      :route="editing"
      :priority="nextPriority(order)"
    />

    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="
        t('routes.confirmDeleteTitle', { name: removing?.name ?? removing?.match.path ?? '' })
      "
      :description="t('routes.confirmDeleteDetail')"
      :confirm-label="t('common.delete')"
      destructive
      :busy="remove.isPending.value"
      @confirm="confirmRemove"
    />
  </div>
</template>
