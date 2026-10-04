<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Play, Power, RotateCw, Square } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { UnitActionName } from '@/api/generated'
import {
  changeGatewayUnitMutation,
  gatewayUnitOptions,
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
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { useSession } from '@/lib/session'
import { REFRESH_INTERVAL_MS, unitStateKey, unitTone } from './presentation'

const { t, d } = useI18n()
const { can } = useSession()
const unit = useQuery({ ...gatewayUnitOptions(), refetchInterval: REFRESH_INTERVAL_MS })
const change = useMutation(changeGatewayUnitMutation())
const status = computed(() => unit.data.value)
const running = computed(() => status.value?.active_state === 'active')

const confirmStop = ref(false)
const confirmRestart = ref(false)

const state = computed(() => {
  const value = status.value
  if (!value) {
    return ''
  }
  const key = unitStateKey(value.active_state)
  const active = key ? t(`host.unit.states.${key}`) : value.active_state
  return value.sub_state ? `${active} (${value.sub_state})` : active
})

const facts = computed(() => {
  const value = status.value
  if (!value) {
    return []
  }
  return [
    {
      label: t('host.unit.since'),
      value: value.active_since ? d(new Date(value.active_since), 'datetime') : '',
    },
    { label: t('host.unit.pid'), value: value.main_pid ? String(value.main_pid) : '' },
    { label: t('host.unit.enabled'), value: value.unit_file_state },
    { label: t('host.unit.restarts'), value: String(value.restarts) },
    { label: t('host.unit.result'), value: value.result },
  ].filter((fact) => fact.value)
})

function run(action: UnitActionName) {
  change.mutate(
    { path: { action }, headers: plainHeaders() },
    {
      onSuccess: () => {
        toast.success(t(`host.unit.done.${action}`))
        confirmStop.value = false
        confirmRestart.value = false
        void unit.refetch()
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
        <Power class="size-4" aria-hidden="true" />{{ t('host.unit.title') }}
      </CardTitle>
      <CardDescription class="font-mono text-xs">{{ status?.name }}</CardDescription>
      <CardAction v-if="status && can('host.manage')" class="flex flex-wrap gap-2">
        <Button
          v-if="!running"
          variant="outline"
          size="sm"
          :disabled="change.isPending.value"
          @click="run('start')"
        >
          <Play data-icon="inline-start" aria-hidden="true" />{{ t('host.unit.start') }}
        </Button>
        <template v-else>
          <Button
            variant="outline"
            size="sm"
            :disabled="change.isPending.value"
            @click="confirmRestart = true"
          >
            <RotateCw data-icon="inline-start" aria-hidden="true" />{{ t('host.unit.restart') }}
          </Button>
          <Button
            variant="outline"
            size="sm"
            :disabled="change.isPending.value"
            @click="confirmStop = true"
          >
            <Square data-icon="inline-start" aria-hidden="true" />{{ t('host.unit.stop') }}
          </Button>
        </template>
      </CardAction>
    </CardHeader>
    <CardContent class="flex flex-col gap-3">
      <ApiFailureAlert
        v-if="unit.isError.value && !status"
        :error="unit.error.value"
        retryable
        @retry="unit.refetch()"
      />
      <Skeleton
        v-else-if="unit.isPending.value"
        class="h-16 rounded-lg"
        aria-busy="true"
        :aria-label="t('state.loading')"
      />
      <template v-else-if="status">
        <StatusIndicator :tone="unitTone(status.active_state)" :label="state" />
        <p v-if="status.description" class="text-muted-foreground text-sm">
          {{ status.description }}
        </p>
        <dl class="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[auto_1fr]">
          <template v-for="fact in facts" :key="fact.label">
            <dt class="text-muted-foreground">{{ fact.label }}</dt>
            <dd class="min-w-0 break-words tabular-nums">{{ fact.value }}</dd>
          </template>
        </dl>
      </template>
    </CardContent>
  </Card>

  <ConfirmDialog
    v-model:open="confirmRestart"
    :icon="RotateCw"
    :title="t('host.unit.confirmRestartTitle')"
    :description="t('host.unit.confirmRestartDetail')"
    :confirm-label="t('host.unit.restart')"
    :busy="change.isPending.value"
    @confirm="run('restart')"
  />
  <ConfirmDialog
    v-model:open="confirmStop"
    :icon="Square"
    :title="t('host.unit.confirmStopTitle')"
    :description="t('host.unit.confirmStopDetail')"
    :confirm-label="t('host.unit.stop')"
    destructive
    :busy="change.isPending.value"
    @confirm="run('stop')"
  />
</template>
