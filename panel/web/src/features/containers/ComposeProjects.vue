<script setup lang="ts">
import { computed, ref } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { FileCode, ScrollText, ShieldCheck, SquareStack } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { ComposeProjectView } from '@/api/generated'
import { listProjectsOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { useSession } from '@/lib/session'
import ComposeActions from './ComposeActions.vue'
import ComposeFilesSheet from './ComposeFilesSheet.vue'
import ComposeLogsSheet from './ComposeLogsSheet.vue'
import { projectCondition, projectTone, REFRESH_INTERVAL_MS } from './presentation'

const props = defineProps<{ engine: string }>()

const { t } = useI18n()
const { can } = useSession()
const projects = useQuery(
  computed(() => ({
    ...listProjectsOptions({ path: { engine: props.engine } }),
    refetchInterval: REFRESH_INTERVAL_MS,
  })),
)
const rows = computed(() => projects.data.value?.projects ?? [])
const actionable = computed(() => can('containers.inspect') || can('containers.manage'))

/** How a project's containers run, in words. */
function described(project: ComposeProjectView) {
  const condition = projectCondition(project)
  return {
    tone: projectTone(condition),
    label: t(`containers.projects.conditions.${condition}`),
    detail: t(
      'containers.projects.running',
      { running: project.running, count: project.containers },
      project.containers,
    ),
  }
}

const chosen = ref<ComposeProjectView>()
const logsOpen = ref(false)
const filesOpen = ref(false)
function readLogs(project: ComposeProjectView) {
  chosen.value = project
  logsOpen.value = true
}
function readFiles(project: ComposeProjectView) {
  chosen.value = project
  filesOpen.value = true
}
</script>

<template>
  <ApiFailureAlert
    v-if="projects.isError.value && !projects.data.value"
    :error="projects.error.value"
    retryable
    @retry="projects.refetch()"
  />
  <Skeleton
    v-else-if="projects.isPending.value"
    class="h-24 rounded-lg"
    aria-busy="true"
    :aria-label="t('state.loading')"
  />
  <div v-else-if="rows.length" class="overflow-x-auto">
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>{{ t('containers.projects.name') }}</TableHead>
          <TableHead class="hidden sm:table-cell">{{ t('containers.list.state') }}</TableHead>
          <TableHead class="hidden md:table-cell">
            {{ t('containers.projects.services') }}
          </TableHead>
          <TableHead v-if="actionable">
            <span class="sr-only">{{ t('common.actions') }}</span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="project in rows" :key="project.name">
          <TableCell>
            <div class="flex flex-col items-start gap-1">
              <span class="flex flex-wrap items-center gap-2">
                <span class="font-medium break-all">{{ project.name }}</span>
                <Badge v-if="project.installation" variant="secondary">
                  <ShieldCheck aria-hidden="true" />{{ t('containers.projects.installation') }}
                </Badge>
              </span>
              <span
                v-if="project.working_directory"
                class="text-muted-foreground font-mono text-xs break-all"
              >
                {{ project.working_directory }}
              </span>
              <span class="flex flex-col gap-0.5 sm:hidden">
                <StatusIndicator
                  :tone="described(project).tone"
                  :label="described(project).label"
                />
                <span class="text-muted-foreground text-xs">{{ described(project).detail }}</span>
              </span>
            </div>
          </TableCell>
          <TableCell class="hidden sm:table-cell">
            <div class="flex flex-col gap-1">
              <StatusIndicator :tone="described(project).tone" :label="described(project).label" />
              <span class="text-muted-foreground text-xs">{{ described(project).detail }}</span>
            </div>
          </TableCell>
          <TableCell class="hidden md:table-cell">
            <ul class="flex flex-col gap-0.5">
              <li
                v-for="service in project.services"
                :key="service.name"
                class="flex items-center gap-2 text-sm whitespace-nowrap"
              >
                <span class="font-mono text-xs">{{ service.name }}</span>
                <span class="text-muted-foreground text-xs tabular-nums">
                  {{ service.running }}/{{ service.containers }}
                </span>
              </li>
            </ul>
          </TableCell>
          <TableCell v-if="actionable">
            <div class="flex items-center justify-end gap-1">
              <template v-if="can('containers.inspect')">
                <Button
                  variant="ghost"
                  size="icon-sm"
                  :aria-label="t('containers.logs.open', { name: project.name })"
                  @click="readLogs(project)"
                >
                  <ScrollText aria-hidden="true" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  :aria-label="t('containers.projects.files', { name: project.name })"
                  @click="readFiles(project)"
                >
                  <FileCode aria-hidden="true" />
                </Button>
              </template>
              <ComposeActions v-if="can('containers.manage')" :engine="engine" :project="project" />
            </div>
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  </div>
  <Empty v-else class="border border-dashed">
    <EmptyHeader>
      <EmptyMedia variant="icon"><SquareStack aria-hidden="true" /></EmptyMedia>
      <EmptyTitle>{{ t('containers.projects.empty') }}</EmptyTitle>
      <EmptyDescription>{{ t('containers.projects.emptyDetail') }}</EmptyDescription>
    </EmptyHeader>
  </Empty>
  <template v-if="chosen">
    <ComposeLogsSheet v-model:open="logsOpen" :engine="engine" :project="chosen" />
    <ComposeFilesSheet v-model:open="filesOpen" :engine="engine" :project="chosen" />
  </template>
</template>
