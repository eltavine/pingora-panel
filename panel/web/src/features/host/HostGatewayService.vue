<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Play, Power, RotateCw, Square } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { GatewayServiceActionName } from '@/api/generated'
import {
  changeGatewayServiceMutation,
  gatewayServiceOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
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
import { engineName, gatewayHealthKey, gatewayTone } from '@/lib/containers'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { useSession } from '@/lib/session'
import { REFRESH_INTERVAL_MS } from './presentation'

const { t, d } = useI18n()
const { can } = useSession()
const service = useQuery({ ...gatewayServiceOptions(), refetchInterval: REFRESH_INTERVAL_MS })
const change = useMutation(changeGatewayServiceMutation())
const gateway = computed(() => service.data.value?.container)
const running = computed(() => gateway.value?.state === 'running')

const confirmStop = ref(false)
const confirmRestart = ref(false)

const state = computed(() => {
  const value = gateway.value
  if (!value) {
    return ''
  }
  const label = t(`containers.states.${value.state}`)
  const health = gatewayHealthKey(value.health)
  return health ? `${label} · ${t(`host.gatewayService.health.${health}`)}` : label
})

const facts = computed(() => {
  const value = gateway.value
  if (!value) {
    return []
  }
  const time = (moment: string | null | undefined) =>
    moment ? d(new Date(moment), 'datetime') : ''
  return [
    { label: t('host.gatewayService.started'), value: time(value.started_at), mono: false },
    { label: t('host.gatewayService.stopped'), value: time(value.finished_at), mono: false },
    {
      label: t('host.gatewayService.exitCode'),
      value:
        value.exit_code === null || value.exit_code === undefined ? '' : String(value.exit_code),
      mono: false,
    },
    { label: t('host.gatewayService.restarts'), value: String(value.restarts), mono: false },
    { label: t('host.gatewayService.status'), value: value.status, mono: false },
    { label: t('host.gatewayService.image'), value: value.image, mono: true },
  ].filter((fact) => fact.value)
})

function run(action: GatewayServiceActionName) {
  change.mutate(
    { path: { action }, headers: plainHeaders() },
    {
      onSuccess: () => {
        toast.success(t(`host.gatewayService.done.${action}`))
        confirmStop.value = false
        confirmRestart.value = false
        void service.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <Card class="min-w-0">
    <CardHeader>
      <CardTitle class="flex items-center gap-2">
        <Power class="size-4" aria-hidden="true" />{{ t('host.gatewayService.title') }}
      </CardTitle>
      <CardDescription v-if="gateway" class="flex flex-wrap items-center gap-x-2 text-xs">
        <span class="font-mono">{{ gateway.name }}</span>
        <span aria-hidden="true">·</span>
        <span>{{ engineName(gateway.engine) }}</span>
      </CardDescription>
      <CardAction v-if="gateway && can('host.manage')" class="flex flex-wrap gap-2">
        <Button
          v-if="!running"
          variant="outline"
          size="sm"
          :disabled="change.isPending.value"
          @click="run('start')"
        >
          <Play data-icon="inline-start" aria-hidden="true" />{{ t('host.gatewayService.start') }}
        </Button>
        <template v-else>
          <Button
            variant="outline"
            size="sm"
            :disabled="change.isPending.value"
            @click="confirmRestart = true"
          >
            <RotateCw data-icon="inline-start" aria-hidden="true" />{{
              t('host.gatewayService.restart')
            }}
          </Button>
          <Button
            variant="outline"
            size="sm"
            :disabled="change.isPending.value"
            @click="confirmStop = true"
          >
            <Square data-icon="inline-start" aria-hidden="true" />{{
              t('host.gatewayService.stop')
            }}
          </Button>
        </template>
      </CardAction>
    </CardHeader>
    <CardContent class="flex flex-col gap-3">
      <ApiFailureAlert
        v-if="service.isError.value && !gateway"
        :error="service.error.value"
        retryable
        @retry="service.refetch()"
      />
      <Skeleton
        v-else-if="service.isPending.value"
        class="h-16 rounded-lg"
        aria-busy="true"
        :aria-label="t('state.loading')"
      />
      <template v-else-if="gateway">
        <StatusIndicator :tone="gatewayTone(gateway.state, gateway.health)" :label="state" />
        <dl class="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[auto_1fr]">
          <template v-for="fact in facts" :key="fact.label">
            <dt class="text-muted-foreground">{{ fact.label }}</dt>
            <dd
              class="min-w-0 break-words tabular-nums"
              :class="{ 'font-mono text-xs': fact.mono }"
            >
              {{ fact.value }}
            </dd>
          </template>
        </dl>
      </template>
    </CardContent>
  </Card>

  <ConfirmDialog
    v-model:open="confirmRestart"
    :icon="RotateCw"
    :title="t('host.gatewayService.confirmRestartTitle')"
    :description="t('host.gatewayService.confirmRestartDetail')"
    :confirm-label="t('host.gatewayService.restart')"
    :busy="change.isPending.value"
    @confirm="run('restart')"
  />
  <ConfirmDialog
    v-model:open="confirmStop"
    :icon="Square"
    :title="t('host.gatewayService.confirmStopTitle')"
    :description="t('host.gatewayService.confirmStopDetail')"
    :confirm-label="t('host.gatewayService.stop')"
    destructive
    :busy="change.isPending.value"
    @confirm="run('stop')"
  />
</template>
