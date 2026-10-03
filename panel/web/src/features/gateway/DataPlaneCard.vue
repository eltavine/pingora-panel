<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Clock, Cpu, GitBranch, Lock, LockOpen, Power, RotateCw, Save, Tag } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import {
  dataPlaneOptions,
  reloadMutation,
  shutdownMutation,
  workersMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
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
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Skeleton } from '@/components/ui/skeleton'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { formatUptime } from './uptime'

const REFRESH_INTERVAL_MS = 10_000

const { t, d } = useI18n()
const plane = useQuery({
  ...dataPlaneOptions(),
  refetchInterval: REFRESH_INTERVAL_MS,
  retry: false,
})
const reload = useMutation(reloadMutation())
const shutdown = useMutation(shutdownMutation())
const workers = useMutation(workersMutation())

const workerCount = ref<number | string>('')
watch(
  () => plane.data.value?.worker_count,
  (count) => {
    if (count !== undefined && workerCount.value === '') {
      workerCount.value = count
    }
  },
  { immediate: true },
)

const confirmReload = ref(false)
const confirmShutdown = ref(false)
const onError = (error: unknown) => notifyFailure(error, t('common.changeFailed'))

function runReload() {
  reload.mutate(
    { headers: plainHeaders() },
    {
      onSuccess: (state) => {
        toast.success(t('gateway.reloaded', { generation: state.generation }))
        confirmReload.value = false
        void plane.refetch()
      },
      onError,
    },
  )
}

function runShutdown() {
  shutdown.mutate(
    { headers: plainHeaders() },
    {
      onSuccess: () => {
        toast.success(t('gateway.shutdownAccepted'))
        confirmShutdown.value = false
      },
      onError,
    },
  )
}

function saveWorkers() {
  const count = Number(workerCount.value)
  workers.mutate(
    { body: { worker_count: count }, headers: plainHeaders() },
    {
      onSuccess: (state) => {
        toast.success(t('gateway.workersSaved', { count: state.worker_count }))
        workerCount.value = state.worker_count
        void plane.refetch()
      },
      onError,
    },
  )
}

const facts = computed(() => {
  const state = plane.data.value
  if (!state) {
    return []
  }
  return [
    { icon: GitBranch, label: t('gateway.generation'), value: String(state.generation) },
    { icon: Clock, label: t('gateway.uptime'), value: formatUptime(state.uptime_seconds) },
    {
      icon: Clock,
      label: t('gateway.startedAt'),
      value: state.started_at ? d(new Date(state.started_at), 'datetime') : t('state.none'),
    },
    { icon: Tag, label: t('gateway.gatewayVersion'), value: state.gateway_version },
    { icon: Tag, label: t('gateway.engineVersion'), value: state.engine_version },
    { icon: Tag, label: t('gateway.adapterVersion'), value: state.adapter_version },
  ]
})
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle class="flex items-center gap-2">
        <Cpu class="size-4" aria-hidden="true" />
        {{ t('gateway.dataPlane') }}
      </CardTitle>
      <CardDescription>{{ t('gateway.dataPlaneDetail') }}</CardDescription>
      <CardAction class="flex flex-wrap gap-2">
        <Button
          variant="outline"
          size="sm"
          :disabled="!plane.data.value"
          @click="confirmReload = true"
        >
          <RotateCw data-icon="inline-start" aria-hidden="true" />
          {{ t('gateway.reload') }}
        </Button>
        <Button
          variant="destructive"
          size="sm"
          :disabled="!plane.data.value"
          @click="confirmShutdown = true"
        >
          <Power data-icon="inline-start" aria-hidden="true" />
          {{ t('gateway.shutdown') }}
        </Button>
      </CardAction>
    </CardHeader>
    <CardContent class="flex flex-col gap-6">
      <ApiFailureAlert
        v-if="plane.isError.value && !plane.data.value"
        :error="plane.error.value"
        retryable
        @retry="plane.refetch()"
      />
      <Skeleton v-else-if="!plane.data.value" class="h-32 w-full" />
      <template v-else>
        <p v-if="plane.data.value.error" class="text-destructive text-sm" role="alert">
          {{ plane.data.value.error }}
        </p>
        <dl class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          <div v-for="fact in facts" :key="fact.label" class="flex min-w-0 flex-col gap-1">
            <dt class="text-muted-foreground flex items-center gap-1.5 text-xs">
              <component :is="fact.icon" class="size-3.5" aria-hidden="true" />
              {{ fact.label }}
            </dt>
            <dd class="truncate font-mono text-sm">{{ fact.value }}</dd>
          </div>
        </dl>

        <form class="flex flex-col gap-1.5" @submit.prevent="saveWorkers">
          <Label for="gateway-workers">{{ t('gateway.workers') }}</Label>
          <div class="flex max-w-xs gap-2">
            <Input
              id="gateway-workers"
              v-model.number="workerCount"
              type="number"
              min="0"
              aria-describedby="gateway-workers-hint"
            />
            <Button
              type="submit"
              variant="outline"
              :disabled="workers.isPending.value || workerCount === ''"
            >
              <Save data-icon="inline-start" aria-hidden="true" />
              {{ t('common.save') }}
            </Button>
          </div>
          <p id="gateway-workers-hint" class="text-muted-foreground text-xs">
            {{ t('gateway.workersHint') }}
          </p>
        </form>

        <section class="flex flex-col gap-2">
          <h3 class="text-sm font-medium">{{ t('gateway.listeners') }}</h3>
          <p v-if="plane.data.value.listeners.length === 0" class="text-muted-foreground text-sm">
            {{ t('gateway.noListeners') }}
          </p>
          <ul v-else class="divide-y rounded-md border">
            <li
              v-for="listener in plane.data.value.listeners"
              :key="listener.id"
              class="flex flex-wrap items-center gap-2 px-3 py-2 text-sm"
            >
              <Lock v-if="listener.tls" class="size-4" :aria-label="t('gateway.tls')" />
              <LockOpen v-else class="size-4" aria-hidden="true" />
              <span class="font-mono text-xs">{{ listener.address }}</span>
              <span class="text-muted-foreground font-mono text-xs">{{ listener.id }}</span>
              <span class="ml-auto flex gap-1">
                <Badge v-if="listener.http1" variant="outline">HTTP/1.1</Badge>
                <Badge v-if="listener.http2" variant="outline">HTTP/2</Badge>
              </span>
            </li>
          </ul>
        </section>
      </template>
    </CardContent>
  </Card>

  <ConfirmDialog
    v-model:open="confirmReload"
    :icon="RotateCw"
    :title="t('gateway.confirmReloadTitle')"
    :description="t('gateway.confirmReloadDetail')"
    :confirm-label="t('gateway.reload')"
    :busy="reload.isPending.value"
    @confirm="runReload"
  />
  <ConfirmDialog
    v-model:open="confirmShutdown"
    :icon="Power"
    :title="t('gateway.confirmShutdownTitle')"
    :description="t('gateway.confirmShutdownDetail')"
    :confirm-label="t('gateway.shutdown')"
    destructive
    :busy="shutdown.isPending.value"
    @confirm="runShutdown"
  />
</template>
