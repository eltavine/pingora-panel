<script setup lang="ts">
import { useQuery } from '@tanstack/vue-query'
import { Boxes, Cpu, RefreshCw } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { servicesOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
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

const { t, d } = useI18n()
const services = useQuery({ ...servicesOptions(), retry: false })
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle class="flex items-center gap-2">
        <Boxes class="size-4" aria-hidden="true" />
        {{ t('gateway.services.title') }}
      </CardTitle>
      <CardDescription>{{ t('gateway.services.description') }}</CardDescription>
      <CardAction>
        <Button
          variant="outline"
          size="sm"
          :disabled="services.isFetching.value"
          @click="services.refetch()"
        >
          <RefreshCw data-icon="inline-start" aria-hidden="true" />
          {{ t('state.refresh') }}
        </Button>
      </CardAction>
    </CardHeader>
    <CardContent>
      <ApiFailureAlert
        v-if="services.isError.value && !services.data.value"
        :error="services.error.value"
        retryable
        @retry="services.refetch()"
      />
      <Skeleton v-else-if="services.isPending.value" class="h-16 w-full" />
      <p
        v-else-if="services.data.value?.services.length === 0"
        class="text-muted-foreground text-sm"
      >
        {{ t('gateway.services.none') }}
      </p>
      <ul v-else-if="services.data.value" class="flex flex-col divide-y">
        <li
          v-for="instance in services.data.value.services"
          :key="instance.instance_id"
          class="flex flex-col gap-1.5 py-3 text-sm first:pt-0 last:pb-0"
        >
          <div class="flex flex-wrap items-center gap-x-3 gap-y-1">
            <Cpu class="size-4 shrink-0" aria-hidden="true" />
            <span class="font-medium">{{ instance.service }}</span>
            <Badge variant="outline" class="font-mono">{{ instance.build_version }}</Badge>
            <span class="text-muted-foreground text-xs">
              {{
                t('gateway.services.started', {
                  time: d(new Date(instance.started_at), 'datetime'),
                })
              }}
            </span>
          </div>
          <span class="text-muted-foreground font-mono text-xs break-all">
            {{ instance.instance_id }}
          </span>
          <div class="flex flex-wrap gap-1.5">
            <Badge
              v-for="protocol in instance.protocols"
              :key="protocol.name"
              variant="secondary"
              class="font-mono"
            >
              {{
                t('gateway.services.protocol', {
                  name: protocol.name,
                  min: protocol.min_revision,
                  max: protocol.max_revision,
                })
              }}
            </Badge>
          </div>
          <span v-if="instance.capabilities.length" class="text-muted-foreground text-xs">
            {{
              t('gateway.services.capabilities', {
                names: instance.capabilities.map((capability) => capability.name).join(', '),
              })
            }}
          </span>
        </li>
      </ul>
    </CardContent>
  </Card>
</template>
