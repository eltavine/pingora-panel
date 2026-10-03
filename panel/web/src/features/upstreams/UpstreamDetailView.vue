<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import {
  ArrowLeft,
  CirclePlay,
  CircleStop,
  Pencil,
  Plus,
  RefreshCw,
  Server,
  Trash2,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { toast } from 'vue-sonner'
import type { UpstreamNode, UpstreamView } from '@/api/generated'
import {
  deleteNodeMutation,
  deleteUpstreamMutation,
  drainMutation,
  getUpstreamOptions,
  getUpstreamQueryKey,
  replaceNodeMutation,
  restoreMutation,
  upstreamHealthOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import {
  changeHeaders,
  notifyFailure,
  plainHeaders,
  useRefreshConfiguration,
} from '@/lib/configuration'
import { algorithmOf, latency, nodeAddress, nodeInputOf, nodeState, nodeTones } from './forms'
import NodeFormSheet from './NodeFormSheet.vue'
import UpstreamFormSheet from './UpstreamFormSheet.vue'

const HEALTH_REFRESH_MS = 3_000

const props = defineProps<{ id: string }>()

const { t, d } = useI18n()
const router = useRouter()
const client = useQueryClient()
const refresh = useRefreshConfiguration()
const upstream = useQuery(computed(() => getUpstreamOptions({ path: { id: props.id } })))
const health = useQuery({
  ...upstreamHealthOptions(),
  refetchInterval: HEALTH_REFRESH_MS,
  retry: false,
})
const replaceNode = useMutation(replaceNodeMutation())
const removeNode = useMutation(deleteNodeMutation())
const removeUpstream = useMutation(deleteUpstreamMutation())
const drain = useMutation(drainMutation())
const restore = useMutation(restoreMutation())

const report = computed(() =>
  health.data.value?.upstreams.find((item) => item.upstream_id === props.id),
)
const endpoints = computed(
  () => new Map((report.value?.nodes ?? []).map((node) => [node.node_id, node])),
)
const observedAt = computed(() =>
  health.dataUpdatedAt.value > 0 ? d(new Date(health.dataUpdatedAt.value), 'time') : null,
)

const onError = (error: unknown) => notifyFailure(error, t('common.changeFailed'))

function saved(updated: UpstreamView) {
  client.setQueryData(getUpstreamQueryKey({ path: { id: props.id } }), updated)
  void refresh()
}

function setEnabled(view: UpstreamView, node: UpstreamNode, enabled: boolean) {
  replaceNode.mutate(
    {
      path: { id: view.id, node: node.id },
      body: { ...nodeInputOf(node), enabled },
      headers: changeHeaders(view.etag),
    },
    { onSuccess: saved, onError },
  )
}

function setDrained(node: UpstreamNode, drained: boolean) {
  const mutation = drained ? drain : restore
  mutation.mutate(
    { path: { id: props.id, node: node.id }, headers: plainHeaders() },
    {
      onSuccess: () => {
        toast.success(drained ? t('upstreams.drainedNotice') : t('upstreams.undrainedNotice'))
        void health.refetch()
      },
      onError,
    },
  )
}

const editingNode = ref<UpstreamNode | undefined>()
const nodeOpen = ref(false)
function openNode(node?: UpstreamNode) {
  editingNode.value = node
  nodeOpen.value = true
}
const editing = ref(false)

const removingNode = ref<UpstreamNode | null>(null)
const removeNodeOpen = computed({
  get: () => removingNode.value !== null,
  set: (open) => {
    if (!open) {
      removingNode.value = null
    }
  },
})
function confirmRemoveNode() {
  const view = upstream.data.value
  const node = removingNode.value
  if (!view || !node) {
    return
  }
  removeNode.mutate(
    { path: { id: view.id, node: node.id }, headers: changeHeaders(view.etag) },
    {
      onSuccess: (updated) => {
        toast.success(t('upstreams.node.removed'))
        removingNode.value = null
        saved(updated)
      },
      onError,
    },
  )
}

const deleting = ref(false)
function confirmDelete() {
  const view = upstream.data.value
  if (!view) {
    return
  }
  removeUpstream.mutate(
    { path: { id: view.id }, headers: changeHeaders(view.etag) },
    {
      onSuccess: () => {
        toast.success(t('upstreams.deleted'))
        void refresh()
        void router.push('/upstreams')
      },
      onError,
    },
  )
}

const facts = computed(() => {
  const view = upstream.data.value
  if (!view) {
    return []
  }
  const none = t('state.none')
  const connection = view.connection ?? {}
  const healthCheck = view.health_check
  const passive = view.passive_health
  const ms = (value: number | null | undefined) => (value == null ? none : `${value} ms`)
  return [
    {
      label: t('upstreams.balancing'),
      value:
        typeof view.balancing === 'object'
          ? `${t('upstreams.algorithms.consistent_hash')} · ${view.balancing.consistent_hash.key}`
          : t(`upstreams.algorithms.${algorithmOf(view.balancing)}`),
    },
    { label: t('upstreams.hostHeader'), value: view.host_header ?? none },
    { label: t('upstreams.connection.connectTimeout'), value: ms(connection.connect_timeout_ms) },
    { label: t('upstreams.connection.readTimeout'), value: ms(connection.read_timeout_ms) },
    { label: t('upstreams.connection.writeTimeout'), value: ms(connection.write_timeout_ms) },
    { label: t('upstreams.connection.idleTimeout'), value: ms(connection.idle_timeout_ms) },
    {
      label: t('upstreams.connection.keepalive'),
      value: (connection.keepalive ?? true) ? t('common.yes') : t('common.no'),
    },
    {
      label: t('upstreams.connection.maxConnections'),
      value: String(connection.max_connections ?? none),
    },
    {
      label: t('upstreams.connection.http2'),
      value: connection.http2 ? t('common.yes') : t('common.no'),
    },
    {
      label: t('upstreams.tls.verifyCertificate'),
      value: (view.tls?.verify_certificate ?? true) ? t('common.yes') : t('common.no'),
    },
    { label: t('upstreams.tls.sni'), value: view.tls?.sni ?? none },
    {
      label: t('upstreams.health.title'),
      value: healthCheck
        ? `${healthCheck.protocol.toUpperCase()} ${healthCheck.protocol === 'http' ? `${healthCheck.method} ${healthCheck.path}` : ''} · ${healthCheck.interval_ms} ms`
        : t('common.disabled'),
    },
    {
      label: t('upstreams.passive.title'),
      value: passive
        ? `${passive.failure_threshold} × · ${passive.ejection_ms} ms`
        : t('common.disabled'),
    },
  ]
})
</script>

<template>
  <div class="flex flex-col gap-6">
    <div>
      <Button variant="ghost" size="sm" as-child>
        <RouterLink to="/upstreams">
          <ArrowLeft data-icon="inline-start" aria-hidden="true" />
          {{ t('common.back') }}
        </RouterLink>
      </Button>
    </div>

    <ApiFailureAlert
      v-if="upstream.isError.value && !upstream.data.value"
      :error="upstream.error.value"
      retryable
      @retry="upstream.refetch()"
    />
    <Skeleton v-else-if="!upstream.data.value" class="h-40 w-full" />

    <template v-else>
      <PageHeader
        :icon="Server"
        :title="upstream.data.value.name"
        :description="
          upstream.data.value.note ??
          t('upstreams.nodeCount', { count: upstream.data.value.nodes?.length ?? 0 })
        "
      >
        <template #actions>
          <Button variant="outline" size="sm" @click="editing = true">
            <Pencil data-icon="inline-start" aria-hidden="true" />
            {{ t('common.edit') }}
          </Button>
          <Button
            variant="destructive"
            size="sm"
            :disabled="upstream.data.value.used_by.length > 0"
            @click="deleting = true"
          >
            <Trash2 data-icon="inline-start" aria-hidden="true" />
            {{ t('common.delete') }}
          </Button>
        </template>
      </PageHeader>

      <Card>
        <CardHeader>
          <CardTitle>{{ t('upstreams.nodes') }}</CardTitle>
          <CardDescription>
            <template v-if="!report">{{ t('upstreams.noLiveData') }}</template>
            <template v-else-if="observedAt">{{
              t('state.updatedAt', { time: observedAt })
            }}</template>
          </CardDescription>
          <CardAction class="flex gap-2">
            <Button
              variant="outline"
              size="icon-sm"
              :aria-label="t('state.refresh')"
              :disabled="health.isFetching.value"
              @click="health.refetch()"
            >
              <RefreshCw :class="{ 'animate-spin': health.isFetching.value }" aria-hidden="true" />
            </Button>
            <Button size="sm" @click="openNode()">
              <Plus data-icon="inline-start" aria-hidden="true" />
              {{ t('upstreams.node.add') }}
            </Button>
          </CardAction>
        </CardHeader>
        <CardContent>
          <div class="rounded-lg border">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{{ t('upstreams.node.address') }}</TableHead>
                  <TableHead>{{ t('common.status') }}</TableHead>
                  <TableHead class="text-right">{{ t('upstreams.node.weight') }}</TableHead>
                  <TableHead class="text-right">{{ t('upstreams.inFlight') }}</TableHead>
                  <TableHead class="text-right">{{ t('upstreams.requests') }}</TableHead>
                  <TableHead class="text-right">{{ t('upstreams.failures') }}</TableHead>
                  <TableHead class="text-right">{{ t('upstreams.latency') }}</TableHead>
                  <TableHead>{{ t('common.enabled') }}</TableHead>
                  <TableHead class="w-28"
                    ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
                  >
                </TableRow>
              </TableHeader>
              <TableBody>
                <TableRow v-for="node in upstream.data.value.nodes ?? []" :key="node.id">
                  <TableCell class="max-w-64">
                    <div class="flex items-center gap-1.5">
                      <span class="truncate font-mono text-xs">{{ nodeAddress(node) }}</span>
                      <Badge v-if="node.backup" variant="outline">{{
                        t('upstreams.node.backup')
                      }}</Badge>
                      <Badge v-if="node.tls" variant="outline">TLS</Badge>
                    </div>
                    <span v-if="node.note" class="text-muted-foreground block truncate text-xs">{{
                      node.note
                    }}</span>
                  </TableCell>
                  <TableCell>
                    <StatusIndicator
                      :tone="nodeTones[nodeState(node, endpoints.get(node.id))]"
                      :label="t(`upstreams.${nodeState(node, endpoints.get(node.id))}`)"
                    />
                  </TableCell>
                  <TableCell class="text-right tabular-nums">{{ node.weight ?? 1 }}</TableCell>
                  <TableCell class="text-right tabular-nums">{{
                    endpoints.get(node.id)?.in_flight ?? '—'
                  }}</TableCell>
                  <TableCell class="text-right tabular-nums">{{
                    endpoints.get(node.id)?.requests ?? '—'
                  }}</TableCell>
                  <TableCell class="text-right tabular-nums">{{
                    endpoints.get(node.id)?.failures ?? '—'
                  }}</TableCell>
                  <TableCell class="text-right tabular-nums">{{
                    latency(endpoints.get(node.id)?.latency_us)
                  }}</TableCell>
                  <TableCell>
                    <Switch
                      :model-value="node.enabled ?? true"
                      :disabled="replaceNode.isPending.value"
                      :aria-label="t('common.enabled')"
                      @update:model-value="setEnabled(upstream.data.value, node, $event)"
                    />
                  </TableCell>
                  <TableCell>
                    <div class="flex justify-end gap-1">
                      <Button
                        v-if="endpoints.get(node.id)?.drained"
                        variant="ghost"
                        size="icon-sm"
                        :aria-label="t('upstreams.undrain')"
                        :title="t('upstreams.undrain')"
                        :disabled="restore.isPending.value"
                        @click="setDrained(node, false)"
                      >
                        <CirclePlay aria-hidden="true" />
                      </Button>
                      <Button
                        v-else
                        variant="ghost"
                        size="icon-sm"
                        :aria-label="t('upstreams.drain')"
                        :title="t('upstreams.drain')"
                        :disabled="!report || drain.isPending.value"
                        @click="setDrained(node, true)"
                      >
                        <CircleStop aria-hidden="true" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        :aria-label="t('common.edit')"
                        @click="openNode(node)"
                      >
                        <Pencil aria-hidden="true" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        :aria-label="t('common.remove')"
                        @click="removingNode = node"
                      >
                        <Trash2 aria-hidden="true" />
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              </TableBody>
            </Table>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>{{ t('upstreams.settings') }}</CardTitle>
        </CardHeader>
        <CardContent>
          <dl class="grid gap-x-6 gap-y-4 sm:grid-cols-2 lg:grid-cols-3">
            <div v-for="fact in facts" :key="fact.label" class="flex min-w-0 flex-col gap-1">
              <dt class="text-muted-foreground text-xs">{{ fact.label }}</dt>
              <dd class="truncate text-sm">{{ fact.value }}</dd>
            </div>
          </dl>
        </CardContent>
      </Card>

      <UpstreamFormSheet v-model:open="editing" :upstream="upstream.data.value" @saved="saved" />
      <NodeFormSheet
        v-model:open="nodeOpen"
        :upstream="upstream.data.value"
        :node="editingNode"
        @saved="saved"
      />

      <ConfirmDialog
        v-model:open="removeNodeOpen"
        :icon="Trash2"
        :title="
          t('upstreams.node.confirmRemoveTitle', {
            address: removingNode ? nodeAddress(removingNode) : '',
          })
        "
        :description="t('upstreams.node.confirmRemoveDetail')"
        :confirm-label="t('common.remove')"
        destructive
        :busy="removeNode.isPending.value"
        @confirm="confirmRemoveNode"
      />
      <ConfirmDialog
        v-model:open="deleting"
        :icon="Trash2"
        :title="t('upstreams.confirmDeleteTitle', { name: upstream.data.value.name })"
        :description="t('upstreams.confirmDeleteDetail')"
        :confirm-label="t('common.delete')"
        destructive
        :busy="removeUpstream.isPending.value"
        @confirm="confirmDelete"
      />
    </template>
  </div>
</template>
