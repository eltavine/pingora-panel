<script setup lang="ts">
import { computed, type Component } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { Container, FolderTree, PlugZap, Power, RadioTower } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { AgentCapabilityName } from '@/api/generated'
import { hostAgentOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import HostDirectories from './HostDirectories.vue'
import { agentTone, capabilityTone, REFRESH_INTERVAL_MS } from './presentation'

const { t } = useI18n()
const agent = useQuery({ ...hostAgentOptions(), refetchInterval: REFRESH_INTERVAL_MS })
const view = computed(() => agent.data.value)

const icons: Record<AgentCapabilityName, Component> = {
  directories: FolderTree,
  listeners: RadioTower,
  gateway_unit: Power,
  containers: Container,
}

const offersDirectories = computed(
  () =>
    view.value?.capabilities.some(
      (capability) => capability.capability === 'directories' && capability.state === 'available',
    ) ?? false,
)
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle class="flex items-center gap-2">
        <PlugZap class="size-4" aria-hidden="true" />{{ t('host.agent.title') }}
      </CardTitle>
      <CardDescription>{{ t('host.agent.description') }}</CardDescription>
    </CardHeader>
    <CardContent class="flex flex-col gap-4">
      <ApiFailureAlert
        v-if="agent.isError.value && !view"
        :error="agent.error.value"
        retryable
        @retry="agent.refetch()"
      />
      <Skeleton
        v-else-if="agent.isPending.value"
        class="h-16 rounded-lg"
        aria-busy="true"
        :aria-label="t('state.loading')"
      />
      <template v-else-if="view">
        <div class="flex flex-wrap items-center gap-x-4 gap-y-1 text-sm">
          <StatusIndicator
            :tone="agentTone(view.status)"
            :label="t(`host.agent.status.${view.status}`)"
          />
          <span v-if="view.build" class="text-muted-foreground">
            {{ t('host.agent.version', { version: view.build }) }}
          </span>
          <span v-if="view.hostname" class="text-muted-foreground font-mono text-xs">
            {{ view.hostname }}
          </span>
        </div>
        <p v-if="view.status !== 'connected'" class="text-muted-foreground text-sm">
          {{ t(`host.agent.hint.${view.status}`) }}
        </p>
        <ul
          v-else
          class="divide-y rounded-lg border"
          :aria-label="t('host.agent.capabilitiesLabel')"
        >
          <li
            v-for="capability in view.capabilities"
            :key="capability.capability"
            class="flex items-start gap-3 px-3 py-2.5"
          >
            <component
              :is="icons[capability.capability]"
              class="text-muted-foreground mt-0.5 size-4 shrink-0"
              aria-hidden="true"
            />
            <div class="flex min-w-0 flex-1 flex-col gap-1">
              <div class="flex flex-wrap items-center justify-between gap-2">
                <span class="text-sm font-medium">
                  {{ t(`host.agent.capabilities.${capability.capability}`) }}
                </span>
                <StatusIndicator
                  :tone="capabilityTone(capability.state)"
                  :label="t(`host.agent.states.${capability.state}`)"
                />
              </div>
              <p
                v-if="capability.detail"
                class="text-muted-foreground font-mono text-xs break-words"
              >
                {{ capability.detail }}
              </p>
            </div>
          </li>
        </ul>
      </template>
    </CardContent>
  </Card>
  <HostDirectories v-if="offersDirectories" />
</template>
