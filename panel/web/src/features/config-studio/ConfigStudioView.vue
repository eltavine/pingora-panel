<script setup lang="ts">
import { computed, nextTick, ref, useTemplateRef, watch } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { lintGutter, linter } from '@codemirror/lint'
import { keymap } from '@codemirror/view'
import { useEventListener, watchDebounced } from '@vueuse/core'
import {
  ArrowDownUp,
  Check,
  CircleCheck,
  FileCode2,
  FileDown,
  FileInput,
  FileJson2,
  FileUp,
  FilePlus2,
  Layers,
  ListTree,
  GitCompareArrows,
  RotateCw,
  Save,
  Trash2,
  TriangleAlert,
  Undo2,
  CircleAlert,
  WandSparkles,
  X,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { onBeforeRouteLeave } from 'vue-router'
import { toast } from 'vue-sonner'
import {
  ast,
  bundle,
  explain,
  importBundle,
  ir,
  type DiagnosticDetails,
  type Explanation,
  type SyntaxNode,
  type SyntaxTree,
} from '@/api/generated'
import { schemaOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import CodeEditor from '@/components/code/CodeEditor.vue'
import { configurationLanguage } from '@/components/code/language'
import { parseSpan, type Place } from '@/components/code/spans'
import DiagnosticList from '@/components/DiagnosticList.vue'
import PageHeader from '@/components/PageHeader.vue'
import { Alert, AlertAction, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { Spinner } from '@/components/ui/spinner'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { toApiFailure } from '@/lib/api'
import { changeHeaders, notifyFailure, useRefreshConfiguration } from '@/lib/configuration'
import { downloadJson } from '@/lib/download'
import { baseContext, directiveCompletion } from './completion'
import ExplainPanel from './ExplainPanel.vue'
import { editorDiagnostics } from './lint'
import NginxImportSheet from './NginxImportSheet.vue'
import OutlineTree from './OutlineTree.vue'
import ReviewSheet from './ReviewSheet.vue'
import { useConfigFiles } from '@/lib/configFiles'
import { isFilePath, ENTRY } from '@/lib/files'

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const config = useConfigFiles()
const schema = useQuery({ ...schemaOptions(), staleTime: Infinity })
const editor = useTemplateRef<InstanceType<typeof CodeEditor>>('editor')
const reviewing = ref(false)
const importing = ref(false)
const naming = ref(false)
const panel = ref<'problems' | 'outline' | 'effective'>('problems')
const outline = ref<SyntaxTree>()
const cursor = ref<Place>()
const explanation = ref<Explanation>()
let explaining = 0
const exporting = ref(false)
const bundleInput = useTemplateRef<HTMLInputElement>('bundleInput')
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

/** Loads converted files into the editor, to review before saving. */
function useImported(files: Record<string, string>) {
  config.files.value = { ...files }
  config.active.value = ENTRY
  toast.success(t('studio.imported'))
}

async function readOutline() {
  if (panel.value !== 'outline' || !config.source.data.value) {
    return
  }
  try {
    const { data } = await ast({
      body: { files: config.files.value, file: config.active.value },
      throwOnError: true,
    })
    outline.value = data
  } catch {
    outline.value = undefined
  }
}
watch(panel, readOutline)
watchDebounced([text, config.active], readOutline, { debounce: 400 })

function selectNode(node: SyntaxNode) {
  const span = parseSpan(node.span)
  if (span) {
    editor.value?.reveal(span.start, span.end)
  }
}

async function readExplanation() {
  if (panel.value !== 'effective' || !config.source.data.value || !cursor.value) {
    return
  }
  const request = ++explaining
  const { line, column } = cursor.value
  try {
    const { data } = await explain({
      body: { files: config.files.value, file: config.active.value, line, column },
      throwOnError: true,
    })
    if (request === explaining) {
      explanation.value = data
    }
  } catch {
    if (request === explaining) {
      explanation.value = undefined
    }
  }
}
watch(panel, readExplanation)
watch(config.active, () => {
  cursor.value = undefined
  explanation.value = undefined
})
watchDebounced([text, cursor], readExplanation, { debounce: 300 })

async function exportSnapshot() {
  exporting.value = true
  try {
    const { data } = await ir({ throwOnError: true })
    downloadJson(`config-ir-v${config.version.value ?? 0}.json`, data)
    toast.success(t('studio.irDownloaded', { version: config.version.value ?? 0 }))
  } catch (error) {
    notifyFailure(error, t('studio.irFailed'))
  } finally {
    exporting.value = false
  }
}

async function exportBundle() {
  exporting.value = true
  try {
    const { data } = await bundle({ throwOnError: true })
    downloadJson(`configuration-v${config.version.value ?? 0}.json`, data)
    toast.success(t('studio.bundleExported', Object.keys(data.files).length))
  } catch (error) {
    notifyFailure(error, t('studio.bundleExportFailed'))
  } finally {
    exporting.value = false
  }
}

/** Replaces the draft with the bundle chosen in the file picker. */
async function importChosenBundle(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file || config.etag.value === undefined) {
    return
  }
  exporting.value = true
  try {
    const body = JSON.parse(await file.text())
    await importBundle({ body, headers: changeHeaders(config.etag.value), throwOnError: true })
    await config.reload()
    void refresh()
    toast.success(t('studio.bundleImported', Object.keys(config.files.value).length))
  } catch (error) {
    notifyFailure(
      error instanceof SyntaxError ? new Error(t('studio.notABundle')) : error,
      t('studio.bundleImportFailed'),
    )
  } finally {
    exporting.value = false
  }
}

/** Opens the file of a `file:line.column` span and selects the span. */
async function revealSpan(value: string | null | undefined) {
  const span = parseSpan(value)
  if (!span || !(span.file in config.files.value)) {
    return
  }
  config.active.value = span.file
  await nextTick()
  editor.value?.reveal(span.start, span.end)
}

function reveal(diagnostic: DiagnosticDetails) {
  void revealSpan(diagnostic.source_span)
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
        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button variant="outline" size="sm" :disabled="exporting">
              <Spinner v-if="exporting" data-icon="inline-start" />
              <ArrowDownUp v-else data-icon="inline-start" aria-hidden="true" />
              {{ t('studio.transfer') }}
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" class="w-60">
            <DropdownMenuItem @select="exportBundle">
              <FileDown aria-hidden="true" />{{ t('studio.exportBundle') }}
            </DropdownMenuItem>
            <DropdownMenuItem :disabled="config.dirty.value" @select="bundleInput?.click()">
              <FileUp aria-hidden="true" />{{ t('studio.importBundle') }}
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem @select="importing = true">
              <FileInput aria-hidden="true" />{{ t('studio.importNginx') }}
            </DropdownMenuItem>
            <DropdownMenuItem @select="exportSnapshot">
              <FileJson2 aria-hidden="true" />{{ t('studio.downloadIr') }}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <input
          ref="bundleInput"
          type="file"
          accept=".json,application/json"
          class="sr-only"
          tabindex="-1"
          aria-hidden="true"
          @change="importChosenBundle"
        />
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
              @cursor="cursor = $event"
            />
          </div>
        </div>

        <Tabs v-model="panel" class="gap-3 rounded-xl border p-4">
          <div class="flex items-center gap-2">
            <TabsList>
              <TabsTrigger value="problems">
                <CircleAlert aria-hidden="true" />
                {{ t('studio.problemsCount', { count: config.problems.value.length }) }}
              </TabsTrigger>
              <TabsTrigger value="outline">
                <ListTree aria-hidden="true" />
                {{ t('studio.outline') }}
              </TabsTrigger>
              <TabsTrigger value="effective">
                <Layers aria-hidden="true" />
                {{ t('studio.effective') }}
              </TabsTrigger>
            </TabsList>
            <Spinner v-if="config.checking.value" class="size-3.5" />
            <Badge v-if="errors" variant="outline" class="ml-auto">
              {{ t('studio.errors', { count: errors }) }}
            </Badge>
          </div>
          <TabsContent value="problems">
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
          </TabsContent>
          <TabsContent value="outline" class="max-h-96 overflow-y-auto">
            <OutlineTree
              v-if="outline?.directives.length"
              :nodes="outline.directives"
              :aria-label="t('studio.outline')"
              @select="selectNode"
            />
            <p v-else class="text-muted-foreground text-sm">{{ t('studio.outlineEmpty') }}</p>
          </TabsContent>
          <TabsContent value="effective" class="max-h-96 overflow-y-auto">
            <ExplainPanel v-if="explanation" :explanation="explanation" @select="revealSpan" />
            <p v-else class="text-muted-foreground text-sm">{{ t('studio.effectiveHint') }}</p>
          </TabsContent>
        </Tabs>
      </div>
    </div>

    <NginxImportSheet v-model:open="importing" @use="useImported" />
    <ReviewSheet
      v-model:open="reviewing"
      :version="config.version.value"
      :dirty="config.dirty.value"
    />
  </div>
</template>
