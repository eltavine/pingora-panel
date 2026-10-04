<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQueryClient } from '@tanstack/vue-query'
import { Container, Power, PowerOff, ShieldAlert } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { ContainerEngineView } from '@/api/generated'
import {
  disableEngineMutation,
  enableEngineMutation,
  listEnginesQueryKey,
} from '@/api/generated/@tanstack/vue-query.gen'
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
import { formatters } from '@/lib/format'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { useSession } from '@/lib/session'
import { conditionTone, engineCondition } from './presentation'
import { engineName } from '@/lib/containers'

defineProps<{ engines: readonly ContainerEngineView[] }>()

const { t, locale } = useI18n()
const { can } = useSession()
const queryClient = useQueryClient()
const enable = useMutation(enableEngineMutation())
const disable = useMutation(disableEngineMutation())
const busy = computed(() => enable.isPending.value || disable.isPending.value)
const format = computed(() => formatters(locale.value))

/** The engine whose disabling waits for confirmation. */
const disabling = ref<string | null>(null)
const confirmOpen = computed({
  get: () => disabling.value !== null,
  set: (open: boolean) => {
    if (!open) {
      disabling.value = null
    }
  },
})

function settled(engine: string, done: 'enabled' | 'disabled') {
  toast.success(t(`containers.engines.done.${done}`, { engine: engineName(engine) }))
  disabling.value = null
  void queryClient.invalidateQueries({ queryKey: listEnginesQueryKey() })
}

function turnOn(engine: string) {
  enable.mutate(
    { path: { engine }, headers: plainHeaders() },
    {
      onSuccess: () => settled(engine, 'enabled'),
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

function turnOff(engine: string) {
  disable.mutate(
    { path: { engine }, headers: plainHeaders() },
    {
      onSuccess: () => settled(engine, 'disabled'),
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

function status(engine: ContainerEngineView): string {
  const condition = engineCondition(engine)
  if (condition === 'reachable' && engine.version) {
    return t('containers.engines.reachableVersion', {
      version: engine.version.version,
      api: engine.version.api_version,
    })
  }
  return t(`containers.engines.conditions.${condition}`)
}

function facts(engine: ContainerEngineView) {
  const info = engine.info
  if (!info) {
    return []
  }
  return [
    {
      label: t('containers.engines.containers'),
      value: t('containers.engines.running', { running: info.running, total: info.containers }),
    },
    { label: t('containers.engines.images'), value: String(info.images) },
    {
      label: t('containers.engines.resources'),
      value: `${t('containers.engines.cpus', { n: info.cpus }, info.cpus)} · ${format.value.bytes(info.memory_bytes)}`,
    },
    { label: t('containers.engines.storage'), value: info.storage_driver },
    { label: t('containers.engines.system'), value: info.operating_system },
  ].filter((fact) => fact.value)
}
</script>

<template>
  <p v-if="can('containers.manage')" class="text-muted-foreground flex items-start gap-2 text-sm">
    <ShieldAlert class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
    {{ t('containers.engines.rootNote') }}
  </p>
  <section class="grid gap-4 lg:grid-cols-2" :aria-label="t('containers.engines.title')">
    <Card v-for="engine in engines" :key="engine.id" class="min-w-0">
      <CardHeader>
        <CardTitle class="flex items-center gap-2">
          <Container class="size-4" aria-hidden="true" />{{ engineName(engine.id) }}
        </CardTitle>
        <CardDescription class="font-mono text-xs break-all">{{ engine.socket }}</CardDescription>
        <CardAction v-if="can('containers.manage')">
          <Button
            v-if="engine.enabled"
            variant="outline"
            size="sm"
            :disabled="busy"
            @click="disabling = engine.id"
          >
            <PowerOff data-icon="inline-start" aria-hidden="true" />{{
              t('containers.engines.disable')
            }}
          </Button>
          <Button v-else variant="outline" size="sm" :disabled="busy" @click="turnOn(engine.id)">
            <Power data-icon="inline-start" aria-hidden="true" />{{
              t('containers.engines.enable')
            }}
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent class="flex flex-col gap-3">
        <StatusIndicator :tone="conditionTone(engineCondition(engine))" :label="status(engine)" />
        <p
          v-if="engineCondition(engine) === 'unreachable' && engine.detail"
          class="text-muted-foreground text-sm"
        >
          {{ engine.detail }}
        </p>
        <p v-else-if="!engine.enabled" class="text-muted-foreground text-sm">
          {{ t('containers.engines.disabledDetail') }}
        </p>
        <dl
          v-if="facts(engine).length"
          class="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[auto_1fr]"
        >
          <template v-for="fact in facts(engine)" :key="fact.label">
            <dt class="text-muted-foreground">{{ fact.label }}</dt>
            <dd class="min-w-0 break-words tabular-nums">{{ fact.value }}</dd>
          </template>
        </dl>
      </CardContent>
    </Card>
  </section>

  <ConfirmDialog
    v-model:open="confirmOpen"
    :icon="PowerOff"
    :title="t('containers.engines.confirmDisableTitle', { engine: engineName(disabling ?? '') })"
    :description="t('containers.engines.confirmDisableDetail')"
    :confirm-label="t('containers.engines.disable')"
    destructive
    :busy="busy"
    @confirm="disabling && turnOff(disabling)"
  />
</template>
