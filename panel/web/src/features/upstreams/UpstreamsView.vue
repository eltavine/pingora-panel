<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Ellipsis, HeartPulse, Pencil, Plus, Server, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { UpstreamView } from '@/api/generated'
import {
  deleteUpstreamMutation,
  listUpstreamsOptions,
  upstreamHealthOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import PageHeader from '@/components/PageHeader.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
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
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { changeHeaders, notifyFailure, useRefreshConfiguration } from '@/lib/configuration'
import { algorithmOf } from './forms'
import UpstreamFormSheet from './UpstreamFormSheet.vue'

const HEALTH_REFRESH_MS = 5_000

const { t, d } = useI18n()
const refresh = useRefreshConfiguration()
const upstreams = useQuery(listUpstreamsOptions())
const health = useQuery({
  ...upstreamHealthOptions(),
  refetchInterval: HEALTH_REFRESH_MS,
  retry: false,
})
const remove = useMutation(deleteUpstreamMutation())

const healthById = computed(
  () => new Map((health.data.value?.upstreams ?? []).map((item) => [item.upstream_id, item])),
)
function healthSummary(upstream: UpstreamView) {
  const report = healthById.value.get(upstream.id)
  if (!report) {
    return null
  }
  return {
    healthy: report.nodes.filter((node) => node.healthy && !node.drained).length,
    total: report.nodes.length,
  }
}

const editing = ref<UpstreamView | undefined>()
const formOpen = ref(false)
function openForm(upstream?: UpstreamView) {
  editing.value = upstream
  formOpen.value = true
}

const removing = ref<UpstreamView | null>(null)
const removeOpen = computed({
  get: () => removing.value !== null,
  set: (open) => {
    if (!open) {
      removing.value = null
    }
  },
})
function confirmRemove() {
  const upstream = removing.value
  if (!upstream) {
    return
  }
  remove.mutate(
    { path: { id: upstream.id }, headers: changeHeaders(upstream.etag) },
    {
      onSuccess: () => {
        toast.success(t('upstreams.deleted'))
        removing.value = null
        void refresh()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="Server"
      :title="t('upstreams.title')"
      :description="t('upstreams.description')"
    >
      <template #actions>
        <Button size="sm" @click="openForm()">
          <Plus data-icon="inline-start" aria-hidden="true" />
          {{ t('upstreams.new') }}
        </Button>
      </template>
    </PageHeader>

    <ApiFailureAlert
      v-if="upstreams.isError.value && !upstreams.data.value"
      :error="upstreams.error.value"
      retryable
      @retry="upstreams.refetch()"
    />
    <div v-else-if="upstreams.isPending.value" class="flex flex-col gap-2">
      <Skeleton v-for="index in 3" :key="index" class="h-12 w-full" />
    </div>

    <Empty v-else-if="(upstreams.data.value ?? []).length === 0" class="border">
      <EmptyHeader>
        <EmptyMedia variant="icon"><Server aria-hidden="true" /></EmptyMedia>
        <EmptyTitle>{{ t('upstreams.emptyTitle') }}</EmptyTitle>
        <EmptyDescription>{{ t('upstreams.emptyDetail') }}</EmptyDescription>
      </EmptyHeader>
      <EmptyContent>
        <Button size="sm" @click="openForm()">
          <Plus data-icon="inline-start" aria-hidden="true" />
          {{ t('upstreams.new') }}
        </Button>
      </EmptyContent>
    </Empty>

    <div v-else class="rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{{ t('common.name') }}</TableHead>
            <TableHead>{{ t('upstreams.balancing') }}</TableHead>
            <TableHead>{{ t('upstreams.nodes') }}</TableHead>
            <TableHead>{{ t('upstreams.healthy') }}</TableHead>
            <TableHead>{{ t('upstreams.sites') }}</TableHead>
            <TableHead>{{ t('common.updated') }}</TableHead>
            <TableHead class="w-12"
              ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="upstream in upstreams.data.value" :key="upstream.id">
            <TableCell class="max-w-64">
              <RouterLink
                :to="`/upstreams/${upstream.id}`"
                class="block truncate font-medium hover:underline"
              >
                {{ upstream.name }}
              </RouterLink>
              <span v-if="upstream.note" class="text-muted-foreground block truncate text-xs">
                {{ upstream.note }}
              </span>
            </TableCell>
            <TableCell>{{
              t(`upstreams.algorithms.${algorithmOf(upstream.balancing)}`)
            }}</TableCell>
            <TableCell class="tabular-nums">{{ upstream.nodes?.length ?? 0 }}</TableCell>
            <TableCell>
              <span v-if="healthSummary(upstream)" class="inline-flex items-center gap-1.5 text-sm">
                <HeartPulse class="size-4" aria-hidden="true" />
                {{ t('upstreams.healthSummary', healthSummary(upstream)!) }}
              </span>
              <span v-else class="text-muted-foreground text-xs">{{
                t('upstreams.unchecked')
              }}</span>
            </TableCell>
            <TableCell>
              <Badge v-if="upstream.used_by.length > 0" variant="secondary">
                {{ t('upstreams.usedBy', { count: upstream.used_by.length }) }}
              </Badge>
              <span v-else class="text-muted-foreground text-xs">{{ t('upstreams.unused') }}</span>
            </TableCell>
            <TableCell class="text-muted-foreground text-xs tabular-nums">
              {{ d(new Date(upstream.updated_at), 'datetime') }}
            </TableCell>
            <TableCell>
              <DropdownMenu>
                <DropdownMenuTrigger as-child>
                  <Button variant="ghost" size="icon-sm" :aria-label="t('common.actions')">
                    <Ellipsis aria-hidden="true" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem @select="openForm(upstream)">
                    <Pencil aria-hidden="true" />
                    {{ t('common.edit') }}
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    variant="destructive"
                    :disabled="upstream.used_by.length > 0"
                    @select="removing = upstream"
                  >
                    <Trash2 aria-hidden="true" />
                    {{ t('common.delete') }}
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <UpstreamFormSheet v-model:open="formOpen" :upstream="editing" />

    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="t('upstreams.confirmDeleteTitle', { name: removing?.name ?? '' })"
      :description="t('upstreams.confirmDeleteDetail')"
      :confirm-label="t('common.delete')"
      destructive
      :busy="remove.isPending.value"
      @confirm="confirmRemove"
    />
  </div>
</template>
