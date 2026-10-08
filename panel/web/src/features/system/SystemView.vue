<script setup lang="ts">
import { computed, ref } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import {
  Boxes,
  Container,
  FileDown,
  Layers,
  ListChecks,
  MonitorCog,
  RefreshCw,
  Stethoscope,
  TriangleAlert,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import { systemDiagnostics } from '@/api/generated'
import {
  systemPreflightOptions,
  systemVersionsOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { notifyFailure } from '@/lib/configuration'
import { downloadJson } from '@/lib/download'
import { useSession } from '@/lib/session'
import {
  bundleName,
  engineName,
  readinessTone,
  REFRESH_INTERVAL_MS,
  shortDigest,
} from './presentation'

const { t, te, d } = useI18n()
const { can } = useSession()
const versions = useQuery({ ...systemVersionsOptions(), refetchInterval: REFRESH_INTERVAL_MS })
const readiness = useQuery({ ...systemPreflightOptions(), refetchInterval: REFRESH_INTERVAL_MS })
const manifest = computed(() => versions.data.value)
const diagnoses = computed(() => can('platform.diagnose'))
const fetching = computed(() => versions.isFetching.value || readiness.isFetching.value)

function refresh() {
  void versions.refetch()
  void readiness.refetch()
}

function known(key: string, fallback: string) {
  return te(key) ? t(key) : fallback
}

const facts = computed(() => {
  const value = manifest.value
  if (!value) {
    return []
  }
  const deployment = value.deployment
  return [
    { label: t('system.versions.release'), value: `${value.release} (${value.commit})` },
    { label: t('system.versions.api'), value: value.api },
    { label: t('system.versions.language'), value: String(value.language) },
    { label: t('system.versions.irSchema'), value: value.ir_schema ?? '—' },
    {
      label: t('system.versions.gateway'),
      value: value.gateway
        ? t('system.versions.gatewayVersions', { ...value.gateway })
        : t('system.versions.unavailable'),
    },
    {
      label: t('system.versions.agent'),
      value: value.agent
        ? t('system.versions.agentVersions', { ...value.agent })
        : t('system.versions.unavailable'),
    },
    {
      label: t('system.versions.deployment'),
      value: deployment
        ? [
            t('system.versions.deployed', {
              action: known(`system.actions.${deployment.action}`, deployment.action),
              engine: engineName(deployment.engine),
              time: d(new Date(deployment.changed_at), 'datetime'),
            }),
            deployment.previous
              ? t('system.versions.previous', { release: deployment.previous })
              : null,
          ]
            .filter(Boolean)
            .join(' · ')
        : t('system.versions.notDeployed'),
    },
  ]
})

const downloading = ref(false)
async function download() {
  downloading.value = true
  try {
    const { data } = await systemDiagnostics({ throwOnError: true })
    const name = bundleName(data.generated_at)
    downloadJson(name, data)
    toast.success(t('system.diagnostics.downloaded', { name }))
  } catch (error) {
    notifyFailure(error, t('system.diagnostics.failed'))
  } finally {
    downloading.value = false
  }
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="MonitorCog"
      :title="t('system.title')"
      :description="t('system.description')"
    >
      <template #actions>
        <Button variant="outline" size="sm" :disabled="fetching" @click="refresh">
          <RefreshCw
            data-icon="inline-start"
            :class="{ 'animate-spin': fetching }"
            aria-hidden="true"
          />
          {{ t('state.refresh') }}
        </Button>
      </template>
    </PageHeader>

    <div class="grid gap-4 xl:grid-cols-[3fr_2fr]">
      <Card>
        <CardHeader>
          <CardTitle class="flex items-center gap-2">
            <Layers class="size-4" aria-hidden="true" />
            {{ t('system.versions.title') }}
          </CardTitle>
          <CardDescription>{{ t('system.versions.description') }}</CardDescription>
        </CardHeader>
        <CardContent class="flex flex-col gap-6">
          <ApiFailureAlert
            v-if="versions.isError.value && !manifest"
            :error="versions.error.value"
            retryable
            @retry="versions.refetch()"
          />
          <Skeleton v-else-if="versions.isPending.value" class="h-40 w-full" />
          <template v-else-if="manifest">
            <dl class="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[auto_1fr]">
              <template v-for="fact in facts" :key="fact.label">
                <dt class="text-muted-foreground">{{ fact.label }}</dt>
                <dd class="font-mono break-all">{{ fact.value }}</dd>
              </template>
            </dl>

            <Alert v-if="manifest.problems.length" role="status">
              <TriangleAlert aria-hidden="true" />
              <AlertTitle>{{ t('system.versions.problems') }}</AlertTitle>
              <AlertDescription>
                <ul class="list-disc pl-4">
                  <li v-for="problem in manifest.problems" :key="problem">{{ problem }}</li>
                </ul>
              </AlertDescription>
            </Alert>

            <section class="flex flex-col gap-2">
              <h2 class="flex items-center gap-2 text-sm font-medium">
                <Boxes class="size-4" aria-hidden="true" />
                {{ t('system.versions.modules') }}
              </h2>
              <p v-if="!manifest.modules.length" class="text-muted-foreground text-sm">
                {{ t('system.versions.noModules') }}
              </p>
              <Table v-else>
                <TableHeader>
                  <TableRow>
                    <TableHead>{{ t('system.versions.module') }}</TableHead>
                    <TableHead>{{ t('system.versions.build') }}</TableHead>
                    <TableHead>{{ t('system.versions.schema') }}</TableHead>
                    <TableHead>{{ t('system.versions.protocols') }}</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  <TableRow v-for="module in manifest.modules" :key="module.instance_id">
                    <TableCell class="font-medium">{{ module.service }}</TableCell>
                    <TableCell class="font-mono">{{ module.build_version }}</TableCell>
                    <TableCell class="font-mono">{{ module.schema_version ?? '—' }}</TableCell>
                    <TableCell>
                      <div class="flex flex-wrap gap-1.5">
                        <Badge
                          v-for="protocol in module.protocols"
                          :key="protocol.name"
                          variant="secondary"
                          class="font-mono"
                        >
                          {{
                            t('system.versions.protocol', {
                              name: protocol.name,
                              min: protocol.min_revision,
                              max: protocol.max_revision,
                            })
                          }}
                        </Badge>
                      </div>
                    </TableCell>
                  </TableRow>
                </TableBody>
              </Table>
            </section>

            <section v-if="manifest.deployment?.images.length" class="flex flex-col gap-2">
              <h2 class="flex items-center gap-2 text-sm font-medium">
                <Container class="size-4" aria-hidden="true" />
                {{ t('system.versions.images') }}
              </h2>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>{{ t('system.versions.service') }}</TableHead>
                    <TableHead>{{ t('system.versions.image') }}</TableHead>
                    <TableHead>{{ t('system.versions.digest') }}</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  <TableRow v-for="image in manifest.deployment.images" :key="image.service">
                    <TableCell class="font-medium">{{ image.service }}</TableCell>
                    <TableCell class="font-mono break-all">{{ image.image }}</TableCell>
                    <TableCell class="font-mono" :title="image.digest ?? undefined">
                      {{
                        image.digest ? shortDigest(image.digest) : t('system.versions.localBuild')
                      }}
                    </TableCell>
                  </TableRow>
                </TableBody>
              </Table>
            </section>
          </template>
        </CardContent>
      </Card>

      <div class="flex flex-col gap-4">
        <Card>
          <CardHeader>
            <CardTitle class="flex items-center gap-2">
              <ListChecks class="size-4" aria-hidden="true" />
              {{ t('system.readiness.title') }}
            </CardTitle>
            <CardDescription>{{ t('system.readiness.description') }}</CardDescription>
          </CardHeader>
          <CardContent class="flex flex-col gap-4">
            <ApiFailureAlert
              v-if="readiness.isError.value && !readiness.data.value"
              :error="readiness.error.value"
              retryable
              @retry="readiness.refetch()"
            />
            <Skeleton v-else-if="readiness.isPending.value" class="h-32 w-full" />
            <template v-else-if="readiness.data.value">
              <StatusIndicator
                :tone="readiness.data.value.ready ? 'positive' : 'negative'"
                :label="
                  readiness.data.value.ready
                    ? t('system.readiness.ready')
                    : t('system.readiness.notReady')
                "
              />
              <ul class="flex flex-col divide-y" :aria-label="t('system.readiness.title')">
                <li
                  v-for="check in readiness.data.value.checks"
                  :key="check.name"
                  class="flex flex-col gap-1 py-3 text-sm first:pt-0 last:pb-0"
                >
                  <div class="flex flex-wrap items-center justify-between gap-2">
                    <span class="font-medium">
                      {{ known(`system.readiness.checks.${check.name}`, check.name) }}
                    </span>
                    <StatusIndicator
                      :tone="readinessTone(check.state)"
                      :label="known(`system.readiness.states.${check.state}`, check.state)"
                    />
                  </div>
                  <span class="text-muted-foreground">{{ check.detail }}</span>
                </li>
              </ul>
              <p class="text-muted-foreground text-xs">{{ t('system.readiness.hint') }}</p>
            </template>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle class="flex items-center gap-2">
              <Stethoscope class="size-4" aria-hidden="true" />
              {{ t('system.diagnostics.title') }}
            </CardTitle>
            <CardDescription>{{ t('system.diagnostics.description') }}</CardDescription>
          </CardHeader>
          <CardContent class="flex flex-col items-start gap-3">
            <template v-if="diagnoses">
              <Button :disabled="downloading" @click="download">
                <FileDown data-icon="inline-start" aria-hidden="true" />
                {{ t('system.diagnostics.download') }}
              </Button>
              <p class="text-muted-foreground text-xs">{{ t('system.diagnostics.withheld') }}</p>
            </template>
            <p v-else class="text-muted-foreground text-sm">{{ t('system.diagnostics.needs') }}</p>
          </CardContent>
        </Card>
      </div>
    </div>
  </div>
</template>
