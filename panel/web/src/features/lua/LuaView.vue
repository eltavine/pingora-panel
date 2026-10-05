<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import {
  Box,
  Braces,
  Database,
  FileCode2,
  GitBranch,
  History,
  Package,
  PowerOff,
  RefreshCw,
  Save,
  TriangleAlert,
  Undo2,
  Workflow,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import { listRevisionsOptions, luaLibraryOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import CodeEditor from '@/components/code/CodeEditor.vue'
import CopyValue from '@/components/CopyValue.vue'
import DiagnosticList from '@/components/DiagnosticList.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatTile from '@/components/StatTile.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from '@/components/ui/empty'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Spinner } from '@/components/ui/spinner'
import { useConfigFiles } from '@/lib/configFiles'
import { toApiFailure } from '@/lib/api'
import { changeHeaders, notifyFailure, useRefreshConfiguration } from '@/lib/configuration'
import { useSession } from '@/lib/session'
import LuaTestPanel from './LuaTestPanel.vue'
import { handlerCount, isFile, PHASES, shortVersion, type Phase } from './presentation'

const DRAFT = 'draft'

const { t } = useI18n()
const { can } = useSession()
const refresh = useRefreshConfiguration()

const revision = ref(DRAFT)
const library = useQuery(
  computed(() =>
    luaLibraryOptions({
      query: revision.value === DRAFT ? {} : { revision: Number(revision.value) },
    }),
  ),
)
const revisions = useQuery(listRevisionsOptions({ query: { limit: 20 } }))

const scripts = computed(() => library.data.value?.scripts ?? [])
const selected = ref<string>()
watch(
  scripts,
  (list) => {
    if (!list.some((script) => script.id === selected.value)) {
      selected.value = list[0]?.id
    }
  },
  { immediate: true },
)
const script = computed(() => scripts.value.find((item) => item.id === selected.value))

const config = useConfigFiles()
const viewingDraft = computed(() => revision.value === DRAFT)
const editable = computed(
  () =>
    viewingDraft.value &&
    script.value !== undefined &&
    isFile(script.value) &&
    can('config.lua') &&
    can('config.write'),
)
const code = computed({
  get: () => {
    const current = script.value
    if (!current) {
      return ''
    }
    return editable.value ? (config.files.value[current.id] ?? current.code) : current.code
  },
  set: (text: string) => {
    if (editable.value && script.value) {
      config.write(script.value.id, text)
    }
  },
})
const dirty = computed(
  () => script.value !== undefined && config.changed.value.has(script.value.id),
)
const phase = computed<Phase | undefined>(() => {
  const used = script.value?.uses[0]?.phase
  return PHASES.find((known) => known === used)
})

function save() {
  if (!config.etag.value || config.save.isPending.value) {
    return
  }
  config.save.mutate(
    { body: { files: config.files.value }, headers: changeHeaders(config.etag.value) },
    {
      onSuccess: (data) => {
        config.adopt(data)
        toast.success(t('lua.saved', { version: data.version }))
        void refresh()
      },
      onError: (error) => {
        const failure = toApiFailure(error)
        if (failure.kind === 'problem' && failure.problem.status === 412) {
          config.stale.value = true
        }
        notifyFailure(error, t('lua.saveFailed'))
      },
    },
  )
}

function revert() {
  if (script.value) {
    config.write(script.value.id, config.saved.value[script.value.id] ?? script.value.code)
  }
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader :icon="Braces" :title="t('lua.title')" :description="t('lua.description')">
      <template #actions>
        <Select v-model="revision">
          <SelectTrigger class="w-44" :aria-label="t('lua.version')">
            <History class="size-4" aria-hidden="true" />
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem :value="DRAFT">{{ t('lua.draft') }}</SelectItem>
            <SelectItem
              v-for="item in revisions.data.value?.items ?? []"
              :key="item.id"
              :value="String(item.id)"
            >
              {{ t('lua.revision', { id: item.id }) }}
            </SelectItem>
          </SelectContent>
        </Select>
        <Button variant="outline" size="sm" as-child>
          <RouterLink to="/config">
            <FileCode2 data-icon="inline-start" aria-hidden="true" />
            {{ t('nav.config') }}
          </RouterLink>
        </Button>
        <Button
          variant="outline"
          size="icon-sm"
          :aria-label="t('lua.refresh')"
          :disabled="library.isFetching.value"
          @click="library.refetch()"
        >
          <RefreshCw aria-hidden="true" />
        </Button>
      </template>
    </PageHeader>

    <ApiFailureAlert
      v-if="library.isError.value && !library.data.value"
      :error="library.error.value"
      retryable
      @retry="library.refetch()"
    />
    <div v-else-if="library.isPending.value" class="grid gap-3 sm:grid-cols-4">
      <Skeleton v-for="index in 4" :key="index" class="h-20" />
    </div>

    <template v-else-if="library.data.value">
      <div class="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <StatTile :icon="FileCode2" :label="t('lua.scripts')" :value="scripts.length" passive />
        <StatTile
          :icon="Workflow"
          :label="t('lua.handlers')"
          :value="handlerCount(scripts)"
          passive
        />
        <StatTile
          :icon="Database"
          :label="t('lua.sharedDicts')"
          :value="library.data.value.shared_dicts.length"
          passive
        />
        <StatTile
          :icon="TriangleAlert"
          :label="t('lua.diagnostics')"
          :value="library.data.value.diagnostics.length"
          passive
        />
      </div>

      <StatusIndicator
        v-if="library.data.value.disabled"
        tone="warning"
        :label="t('lua.disabled')"
      />
      <DiagnosticList
        v-if="library.data.value.diagnostics.length"
        :diagnostics="library.data.value.diagnostics"
      />

      <Empty v-if="scripts.length === 0" class="border">
        <EmptyHeader>
          <EmptyMedia variant="icon"><Braces aria-hidden="true" /></EmptyMedia>
          <EmptyTitle>{{ t('lua.emptyTitle') }}</EmptyTitle>
          <EmptyDescription>{{ t('lua.emptyDetail') }}</EmptyDescription>
        </EmptyHeader>
        <EmptyContent>
          <Button size="sm" as-child>
            <RouterLink to="/config">
              <FileCode2 data-icon="inline-start" aria-hidden="true" />
              {{ t('nav.config') }}
            </RouterLink>
          </Button>
        </EmptyContent>
      </Empty>

      <div v-else class="grid gap-4 lg:grid-cols-[18rem_minmax(0,1fr)]">
        <nav class="flex flex-col gap-1" :aria-label="t('lua.scripts')">
          <button
            v-for="item in scripts"
            :key="item.id"
            type="button"
            class="hover:bg-accent flex flex-col items-start gap-1 rounded-md border px-3 py-2 text-left"
            :class="item.id === selected ? 'border-foreground bg-accent' : ''"
            :aria-current="item.id === selected ? 'true' : undefined"
            @click="selected = item.id"
          >
            <span class="flex w-full items-center gap-2 font-mono text-xs">
              <component
                :is="isFile(item) ? FileCode2 : Box"
                class="size-3.5 shrink-0"
                aria-hidden="true"
              />
              <span class="truncate">{{ item.id }}</span>
            </span>
            <span class="flex flex-wrap gap-1">
              <Badge
                v-for="(use, index) in item.uses"
                :key="index"
                variant="secondary"
                class="font-mono text-[10px]"
              >
                {{ use.phase }}
              </Badge>
              <Badge v-if="item.module" variant="outline" class="font-mono text-[10px]">
                <Package aria-hidden="true" />
                {{ item.module }}
              </Badge>
            </span>
          </button>
        </nav>

        <div v-if="script" class="flex min-w-0 flex-col gap-4">
          <Card>
            <CardHeader>
              <CardTitle class="flex flex-wrap items-center gap-2 font-mono text-sm">
                <component
                  :is="isFile(script) ? FileCode2 : Box"
                  class="size-4"
                  aria-hidden="true"
                />
                {{ script.id }}
                <Badge v-if="dirty" variant="outline">{{ t('lua.unsaved') }}</Badge>
              </CardTitle>
              <CardDescription class="flex flex-wrap items-center gap-x-4 gap-y-1">
                <span class="flex items-center gap-1">
                  <GitBranch class="size-3.5" aria-hidden="true" />
                  {{ t('lua.versionOf', { version: shortVersion(script.sha256) }) }}
                  <CopyValue :value="script.sha256" />
                </span>
                <span>{{ t('lua.size', { lines: script.lines, bytes: script.bytes }) }}</span>
                <span v-if="!isFile(script)">
                  {{ t('lua.writtenAt', { file: script.file, line: script.line }) }}
                </span>
              </CardDescription>
            </CardHeader>
            <CardContent class="flex flex-col gap-3">
              <ul v-if="script.uses.length" class="flex flex-col gap-1 text-sm">
                <li
                  v-for="(use, index) in script.uses"
                  :key="index"
                  class="flex items-center gap-2"
                >
                  <Workflow class="size-3.5 shrink-0" aria-hidden="true" />
                  <span class="font-mono text-xs">{{ use.phase }}</span>
                  <span class="text-muted-foreground">{{ use.label }}</span>
                </li>
              </ul>
              <p v-else class="text-muted-foreground text-sm">{{ t('lua.notRun') }}</p>
              <p v-if="script.requires.length" class="flex flex-wrap items-center gap-1 text-xs">
                <Package class="size-3.5" aria-hidden="true" />
                {{ t('lua.requires') }}
                <Badge
                  v-for="name in script.requires"
                  :key="name"
                  variant="outline"
                  class="font-mono text-[10px]"
                >
                  {{ name }}
                </Badge>
              </p>
              <div class="h-80 overflow-hidden rounded-md border">
                <CodeEditor
                  v-model="code"
                  :path="`lua:${revision}:${script.id}`"
                  :label="t('lua.editor', { script: script.id })"
                  :readonly="!editable"
                  lua
                />
              </div>
              <div v-if="editable" class="flex flex-wrap items-center gap-2">
                <Button size="sm" :disabled="!dirty || config.save.isPending.value" @click="save">
                  <Spinner v-if="config.save.isPending.value" data-icon="inline-start" />
                  <Save v-else data-icon="inline-start" aria-hidden="true" />
                  {{ t('common.save') }}
                </Button>
                <Button size="sm" variant="outline" :disabled="!dirty" @click="revert">
                  <Undo2 data-icon="inline-start" aria-hidden="true" />
                  {{ t('lua.revert') }}
                </Button>
                <span v-if="config.stale.value" class="text-muted-foreground text-xs">
                  {{ t('lua.stale') }}
                </span>
              </div>
              <p v-else-if="viewingDraft && isFile(script)" class="text-muted-foreground text-xs">
                <PowerOff class="inline size-3.5" aria-hidden="true" />
                {{ t('lua.readOnly') }}
              </p>
            </CardContent>
          </Card>

          <LuaTestPanel v-if="can('config.lua') && viewingDraft" :code="code" :phase="phase" />
        </div>
      </div>
    </template>
  </div>
</template>
