<script setup lang="ts">
import { computed, nextTick, ref, useTemplateRef } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { lintGutter, linter } from '@codemirror/lint'
import { keymap } from '@codemirror/view'
import { useEventListener } from '@vueuse/core'
import {
  Check,
  CircleCheck,
  FileCode2,
  FilePlus2,
  GitCompareArrows,
  RotateCw,
  Save,
  Trash2,
  TriangleAlert,
  Undo2,
  WandSparkles,
  X,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { onBeforeRouteLeave } from 'vue-router'
import { toast } from 'vue-sonner'
import type { DiagnosticDetails } from '@/api/generated'
import { schemaOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import CodeEditor from '@/components/code/CodeEditor.vue'
import { configurationLanguage } from '@/components/code/language'
import { parseSpan } from '@/components/code/spans'
import DiagnosticList from '@/components/DiagnosticList.vue'
import PageHeader from '@/components/PageHeader.vue'
import { Alert, AlertAction, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { Spinner } from '@/components/ui/spinner'
import { toApiFailure } from '@/lib/api'
import { changeHeaders, notifyFailure, useRefreshConfiguration } from '@/lib/configuration'
import { baseContext, directiveCompletion } from './completion'
import { editorDiagnostics } from './lint'
import ReviewSheet from './ReviewSheet.vue'
import { ENTRY, isFilePath, useConfigFiles } from './useConfigFiles'

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const config = useConfigFiles()
const schema = useQuery({ ...schemaOptions(), staleTime: Infinity })
const editor = useTemplateRef<InstanceType<typeof CodeEditor>>('editor')
const reviewing = ref(false)
const naming = ref(false)
const newPath = ref('')

const text = computed({
  get: () => config.files.value[config.active.value] ?? '',
  set: (value: string) => config.write(config.active.value, value),
})
const errors = computed(
  () => config.problems.value.filter((item) => item.severity === 'ERROR').length,
)
const pathError = computed(() => {
  const path = newPath.value.trim()
  if (!path) {
    return ''
  }
  if (!isFilePath(path)) {
    return t('studio.invalidPath')
  }
  return path in config.files.value ? t('studio.exists') : ''
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
        toast.success(t('studio.saved', { version: data.version }))
        void refresh()
      },
      onError: (error) => {
        const failure = toApiFailure(error)
        if (failure.kind === 'problem' && failure.problem.status === 412) {
          config.stale.value = true
        }
        if (failure.kind === 'problem' && failure.problem.field_errors?.length) {
          config.problems.value = failure.problem.field_errors
        }
        notifyFailure(error, t('studio.saveFailed'))
      },
    },
  )
}

function format() {
  config.format.mutate(
    { body: { files: config.files.value } },
    {
      onSuccess: (result) => {
        config.files.value = { ...config.files.value, ...result.files }
        if (result.diagnostics.length) {
          config.problems.value = result.diagnostics
          toast.warning(t('studio.formatSkipped', { count: result.diagnostics.length }))
        } else {
          toast.success(t('studio.formatted'))
        }
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

function addFile() {
  const path = newPath.value.trim()
  if (!path || pathError.value) {
    return
  }
  config.add(path)
  newPath.value = ''
  naming.value = false
}

async function reveal(diagnostic: DiagnosticDetails) {
  const span = parseSpan(diagnostic.source_span)
  if (!span || !(span.file in config.files.value)) {
    return
  }
  config.active.value = span.file
  await nextTick()
  editor.value?.reveal(span.start, span.end)
}

const extensions = [
  configurationLanguage.data.of({
    autocomplete: directiveCompletion(
      () => schema.data.value?.directives,
      () =>
        baseContext(config.active.value, config.files.value, schema.data.value?.directives ?? []),
    ),
  }),
  linter(
    async (view) => editorDiagnostics(await config.lint(), config.active.value, view.state.doc),
    { delay: 500 },
  ),
  lintGutter(),
  keymap.of([
    {
      key: 'Mod-s',
      preventDefault: true,
      run: () => {
        save()
        return true
      },
    },
  ]),
]

onBeforeRouteLeave(() => !config.dirty.value || window.confirm(t('studio.leave')))
useEventListener(window, 'beforeunload', (event: BeforeUnloadEvent) => {
  if (config.dirty.value) {
    event.preventDefault()
  }
})
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader :icon="FileCode2" :title="t('studio.title')" :description="t('studio.description')">
      <template #actions>
        <Button
          variant="outline"
          size="sm"
          :disabled="config.format.isPending.value"
          @click="format"
        >
          <Spinner v-if="config.format.isPending.value" data-icon="inline-start" />
          <WandSparkles v-else data-icon="inline-start" aria-hidden="true" />
          {{ t('studio.format') }}
        </Button>
        <Button variant="outline" size="sm" :disabled="!config.dirty.value" @click="config.revert">
          <Undo2 data-icon="inline-start" aria-hidden="true" />
          {{ t('studio.revert') }}
        </Button>
        <Button
          variant="outline"
          size="sm"
          :disabled="!config.dirty.value || config.save.isPending.value"
          @click="save"
        >
          <Spinner v-if="config.save.isPending.value" data-icon="inline-start" />
          <Save v-else data-icon="inline-start" aria-hidden="true" />
          {{ t('studio.save') }}
        </Button>
        <Button size="sm" @click="reviewing = true">
          <GitCompareArrows data-icon="inline-start" aria-hidden="true" />
          {{ t('studio.review') }}
        </Button>
      </template>
    </PageHeader>

    <Alert v-if="config.stale.value">
      <TriangleAlert aria-hidden="true" />
      <AlertTitle>{{ t('studio.stale') }}</AlertTitle>
      <AlertDescription>{{ t('studio.staleDetail') }}</AlertDescription>
      <AlertAction>
        <Button variant="outline" size="sm" @click="config.reload">
          <RotateCw data-icon="inline-start" aria-hidden="true" />
          {{ t('studio.reload') }}
        </Button>
      </AlertAction>
    </Alert>

    <ApiFailureAlert
      v-if="config.source.isError.value && !config.source.data.value"
      :error="config.source.error.value"
      retryable
      @retry="config.source.refetch()"
    />
    <Skeleton v-else-if="!config.source.data.value" class="h-96 w-full" />

    <div v-else class="grid gap-4 lg:grid-cols-[15rem_minmax(0,1fr)]">
      <Card class="gap-3 self-start py-4">
        <CardHeader class="flex flex-row items-center justify-between px-4">
          <CardTitle class="text-sm">{{ t('studio.files') }}</CardTitle>
          <Button
            variant="ghost"
            size="icon-sm"
            :aria-label="t('studio.newFile')"
            @click="naming = !naming"
          >
            <FilePlus2 aria-hidden="true" />
          </Button>
        </CardHeader>
        <CardContent class="flex flex-col gap-1 px-2">
          <form v-if="naming" class="flex flex-col gap-1 px-2 pb-2" @submit.prevent="addFile">
            <div class="flex items-center gap-1">
              <Input
                v-model="newPath"
                class="h-8 font-mono text-xs"
                placeholder="sites/blog.conf"
                :aria-label="t('studio.path')"
                :aria-invalid="pathError !== ''"
                autofocus
              />
              <Button
                type="submit"
                variant="ghost"
                size="icon-sm"
                :disabled="!newPath.trim() || pathError !== ''"
                :aria-label="t('common.create')"
              >
                <Check aria-hidden="true" />
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                :aria-label="t('common.cancel')"
                @click="naming = false"
              >
                <X aria-hidden="true" />
              </Button>
            </div>
            <p class="text-muted-foreground text-xs" :role="pathError ? 'alert' : undefined">
              {{ pathError || t('studio.newFileHint') }}
            </p>
          </form>
          <ul class="flex flex-col gap-0.5" :aria-label="t('studio.files')">
            <li v-for="path in config.paths.value" :key="path" class="group flex items-center">
              <button
                type="button"
                class="hover:bg-muted flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm"
                :class="{ 'bg-muted font-medium': path === config.active.value }"
                :aria-current="path === config.active.value ? 'true' : undefined"
                @click="config.active.value = path"
              >
                <FileCode2 class="size-4 shrink-0" aria-hidden="true" />
                <span class="truncate font-mono text-xs">{{ path }}</span>
                <span
                  v-if="config.changed.value.has(path)"
                  class="bg-foreground ml-auto size-1.5 shrink-0 rounded-full"
                  role="img"
                  :aria-label="t('studio.unsaved')"
                />
              </button>
              <Button
                v-if="path !== ENTRY"
                variant="ghost"
                size="icon-xs"
                class="opacity-60 group-hover:opacity-100 focus-visible:opacity-100"
                :aria-label="t('studio.deleteFile', { path })"
                @click="config.remove(path)"
              >
                <Trash2 aria-hidden="true" />
              </Button>
            </li>
          </ul>
        </CardContent>
      </Card>

      <div class="flex min-w-0 flex-col gap-4">
        <div class="flex min-w-0 flex-col overflow-hidden rounded-lg border">
          <div class="bg-muted/40 flex flex-wrap items-center gap-2 border-b px-3 py-2 text-sm">
            <FileCode2 class="size-4" aria-hidden="true" />
            <span class="truncate font-mono text-xs">{{ config.active.value }}</span>
            <Badge v-if="config.active.value === ENTRY" variant="outline">{{
              t('studio.entry')
            }}</Badge>
            <Badge v-if="config.changed.value.has(config.active.value)" variant="secondary">
              {{ t('studio.unsaved') }}
            </Badge>
            <span class="text-muted-foreground ml-auto text-xs tabular-nums">
              {{ t('studio.draftVersion', { version: config.version.value ?? 0 }) }}
            </span>
          </div>
          <div class="h-[min(68vh,46rem)] min-h-80">
            <CodeEditor
              ref="editor"
              v-model="text"
              :path="config.active.value"
              :label="t('studio.editorLabel', { path: config.active.value })"
              :extensions="extensions"
            />
          </div>
        </div>

        <Card class="gap-3 py-4">
          <CardHeader class="flex flex-row items-center gap-2 px-4">
            <CardTitle class="text-sm">
              {{ t('studio.problemsCount', { count: config.problems.value.length }) }}
            </CardTitle>
            <Spinner v-if="config.checking.value" class="size-3.5" />
            <Badge v-if="errors" variant="outline" class="ml-auto">
              {{ t('studio.errors', { count: errors }) }}
            </Badge>
          </CardHeader>
          <CardContent class="px-4">
            <DiagnosticList
              v-if="config.problems.value.length"
              :diagnostics="config.problems.value"
              selectable
              @select="reveal"
            />
            <p v-else class="text-muted-foreground flex items-center gap-2 text-sm">
              <CircleCheck class="size-4" aria-hidden="true" />
              {{ t('studio.noProblems') }}
            </p>
          </CardContent>
        </Card>
      </div>
    </div>

    <ReviewSheet
      v-model:open="reviewing"
      :version="config.version.value"
      :dirty="config.dirty.value"
    />
  </div>
</template>
