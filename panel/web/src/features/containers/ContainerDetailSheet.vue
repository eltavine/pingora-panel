<script setup lang="ts">
import { computed } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { Box, HardDrive, Network, Tags } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { ContainerView } from '@/api/generated'
import { inspectContainerOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Badge } from '@/components/ui/badge'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { gatewayHealthKey, gatewayTone } from '@/features/host/presentation'
import { engineName, sortedLabels } from './presentation'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ engine: string; container: ContainerView }>()

const { t, d } = useI18n()
const detail = useQuery(
  computed(() => ({
    ...inspectContainerOptions({
      path: { engine: props.engine, container: props.container.id },
    }),
    enabled: open.value,
  })),
)
const view = computed(() => detail.data.value)
const name = computed(() => props.container.names[0] ?? props.container.id.slice(0, 12))

const state = computed(() => {
  const value = view.value
  if (!value) {
    return ''
  }
  const label = t(`containers.states.${value.container.state}`)
  const health = gatewayHealthKey(value.health)
  return health ? `${label} · ${t(`host.gatewayService.health.${health}`)}` : label
})

const facts = computed(() => {
  const value = view.value
  if (!value) {
    return []
  }
  const time = (moment: string | null | undefined) =>
    moment ? d(new Date(moment), 'datetime') : ''
  const policy = value.restart_policy
    ? value.restart_retries > 0
      ? t('containers.detail.retries', { policy: value.restart_policy, n: value.restart_retries })
      : value.restart_policy
    : ''
  return [
    { label: t('containers.list.image'), value: value.container.image, mono: true },
    { label: t('containers.detail.started'), value: time(value.started_at), mono: false },
    { label: t('containers.detail.stopped'), value: time(value.finished_at), mono: false },
    {
      label: t('containers.detail.exitCode'),
      value:
        value.exit_code === null || value.exit_code === undefined ? '' : String(value.exit_code),
      mono: false,
    },
    { label: t('containers.detail.error'), value: value.error ?? '', mono: false },
    {
      label: t('containers.detail.oomKilled'),
      value: value.oom_killed ? t('containers.detail.yes') : '',
      mono: false,
    },
    { label: t('containers.detail.restarts'), value: String(value.restarts), mono: false },
    { label: t('containers.detail.restartPolicy'), value: policy, mono: true },
    { label: t('containers.detail.hostname'), value: value.hostname ?? '', mono: true },
    { label: t('containers.detail.user'), value: value.user ?? '', mono: true },
    {
      label: t('containers.detail.workingDirectory'),
      value: value.working_directory ?? '',
      mono: true,
    },
    { label: t('containers.detail.platform'), value: value.platform ?? '', mono: true },
  ].filter((fact) => fact.value)
})

const labels = computed(() => sortedLabels(view.value?.container.labels ?? {}))
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-xl">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2 break-all">
          <Box class="size-5 shrink-0" aria-hidden="true" />{{ name }}
        </SheetTitle>
        <SheetDescription class="flex flex-wrap items-center gap-x-2">
          <span class="font-mono text-xs">{{ container.id.slice(0, 12) }}</span>
          <span aria-hidden="true">·</span>
          <span>{{ engineName(engine) }}</span>
        </SheetDescription>
      </SheetHeader>
      <div class="flex flex-col gap-6 px-4 pb-6">
        <ApiFailureAlert
          v-if="detail.isError.value && !view"
          :error="detail.error.value"
          retryable
          @retry="detail.refetch()"
        />
        <Skeleton
          v-else-if="detail.isPending.value"
          class="h-40 rounded-lg"
          aria-busy="true"
          :aria-label="t('state.loading')"
        />
        <template v-else-if="view">
          <StatusIndicator :tone="gatewayTone(view.container.state, view.health)" :label="state" />
          <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
            <template v-for="fact in facts" :key="fact.label">
              <dt class="text-muted-foreground">{{ fact.label }}</dt>
              <dd class="min-w-0 break-words" :class="{ 'font-mono text-xs': fact.mono }">
                {{ fact.value }}
              </dd>
            </template>
          </dl>

          <section class="flex flex-col gap-2">
            <h3 class="flex items-center gap-2 text-sm font-medium">
              <Tags class="size-4" aria-hidden="true" />{{ t('containers.detail.labels') }}
            </h3>
            <dl v-if="labels.length" class="flex flex-col gap-1.5">
              <div v-for="label in labels" :key="label.name" class="flex min-w-0 flex-col">
                <dt class="text-muted-foreground font-mono text-xs break-all">{{ label.name }}</dt>
                <dd class="font-mono text-xs break-all">{{ label.value || '—' }}</dd>
              </div>
            </dl>
            <p v-else class="text-muted-foreground text-sm">
              {{ t('containers.detail.noLabels') }}
            </p>
          </section>

          <section class="flex flex-col gap-2">
            <h3 class="flex items-center gap-2 text-sm font-medium">
              <HardDrive class="size-4" aria-hidden="true" />{{ t('containers.detail.mounts') }}
            </h3>
            <ul v-if="view.mounts.length" class="flex flex-col gap-2">
              <li
                v-for="mount in view.mounts"
                :key="mount.destination"
                class="flex min-w-0 flex-col gap-1 rounded-md border p-2"
              >
                <span class="flex flex-wrap items-center gap-2">
                  <Badge variant="secondary">{{ mount.kind }}</Badge>
                  <Badge variant="outline">
                    {{
                      mount.read_write
                        ? t('containers.detail.readWrite')
                        : t('containers.detail.readOnly')
                    }}
                  </Badge>
                  <span class="font-mono text-xs break-all">{{ mount.destination }}</span>
                </span>
                <span class="text-muted-foreground font-mono text-xs break-all">
                  {{ mount.name ?? mount.source }}
                </span>
              </li>
            </ul>
            <p v-else class="text-muted-foreground text-sm">
              {{ t('containers.detail.noMounts') }}
            </p>
          </section>

          <section class="flex flex-col gap-2">
            <h3 class="flex items-center gap-2 text-sm font-medium">
              <Network class="size-4" aria-hidden="true" />{{ t('containers.detail.networks') }}
            </h3>
            <ul v-if="view.networks.length" class="flex flex-col gap-2">
              <li
                v-for="network in view.networks"
                :key="network.name"
                class="flex min-w-0 flex-col gap-1 rounded-md border p-2 text-sm"
              >
                <span class="font-medium break-all">{{ network.name }}</span>
                <span v-if="network.ip_address" class="font-mono text-xs">
                  {{ network.ip_address }}
                  <span v-if="network.gateway" class="text-muted-foreground">
                    · {{ t('containers.detail.gateway', { gateway: network.gateway }) }}
                  </span>
                </span>
                <span v-if="network.aliases.length" class="text-muted-foreground text-xs break-all">
                  {{ t('containers.detail.aliases', { aliases: network.aliases.join(', ') }) }}
                </span>
              </li>
            </ul>
            <p v-else class="text-muted-foreground text-sm">
              {{ t('containers.detail.noNetworks') }}
            </p>
          </section>
        </template>
      </div>
    </SheetContent>
  </Sheet>
</template>
