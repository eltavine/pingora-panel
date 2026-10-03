<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { FileInput, FolderOpen, Files, RefreshCw, TriangleAlert } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { NginxImportResponse } from '@/api/generated'
import { importNginxMutation } from '@/api/generated/@tanstack/vue-query.gen'
import CodeEditor from '@/components/code/CodeEditor.vue'
import DiagnosticList from '@/components/DiagnosticList.vue'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Spinner } from '@/components/ui/spinner'
import { Textarea } from '@/components/ui/textarea'
import { notifyFailure } from '@/lib/configuration'

const PASTED = 'nginx.conf'

const open = defineModel<boolean>('open', { required: true })
const emit = defineEmits<{ use: [files: Record<string, string>] }>()

const { t } = useI18n()
const pasted = ref('')
const chosen = ref<Record<string, string>>({})
const entry = ref(PASTED)
const result = ref<NginxImportResponse>()
const convert = useMutation(importNginxMutation())

const files = computed(() =>
  Object.keys(chosen.value).length ? chosen.value : { [PASTED]: pasted.value },
)
const entries = computed(() => Object.keys(files.value).sort())

watch(open, (value) => {
  if (value) {
    result.value = undefined
  }
})

/** Reads picked files; a folder keeps each file's path within it. */
async function pick(event: Event) {
  const input = event.target as HTMLInputElement
  const picked: Record<string, string> = {}
  for (const file of Array.from(input.files ?? [])) {
    picked[file.webkitRelativePath || file.name] = await file.text()
  }
  input.value = ''
  if (!Object.keys(picked).length) {
    return
  }
  chosen.value = picked
  const names = Object.keys(picked)
  entry.value =
    names.find((name) => name === 'nginx.conf' || name.endsWith('/nginx.conf')) ?? names.sort()[0]!
  result.value = undefined
}

function run() {
  convert.mutate(
    { body: { files: files.value, entry: entry.value } },
    {
      onSuccess: (data) => (result.value = data),
      onError: (error) => notifyFailure(error, t('studio.importFailed')),
    },
  )
}

function use() {
  if (result.value) {
    emit('use', result.value.files)
    open.value = false
  }
}

function clearChosen() {
  chosen.value = {}
  entry.value = PASTED
  result.value = undefined
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-3xl">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2">
          <FileInput class="size-5" aria-hidden="true" />
          {{ t('studio.importNginx') }}
        </SheetTitle>
        <SheetDescription>{{ t('studio.importDetail') }}</SheetDescription>
      </SheetHeader>

      <div class="flex flex-col gap-4 px-4">
        <div class="flex flex-wrap items-center gap-2">
          <Button variant="outline" size="sm" as-child>
            <label class="cursor-pointer">
              <Files data-icon="inline-start" aria-hidden="true" />
              {{ t('studio.chooseFiles') }}
              <input type="file" multiple class="sr-only" @change="pick" />
            </label>
          </Button>
          <Button variant="outline" size="sm" as-child>
            <label class="cursor-pointer">
              <FolderOpen data-icon="inline-start" aria-hidden="true" />
              {{ t('studio.chooseFolder') }}
              <input type="file" webkitdirectory class="sr-only" @change="pick" />
            </label>
          </Button>
          <Button v-if="Object.keys(chosen).length" variant="ghost" size="sm" @click="clearChosen">
            {{ t('studio.pasteInstead') }}
          </Button>
        </div>

        <div v-if="Object.keys(chosen).length" class="flex flex-col gap-1.5">
          <Label for="nginx-entry">{{ t('studio.entryFile') }}</Label>
          <Select v-model="entry">
            <SelectTrigger id="nginx-entry" class="w-full font-mono text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="name in entries" :key="name" :value="name">{{ name }}</SelectItem>
            </SelectContent>
          </Select>
          <p class="text-muted-foreground text-xs">
            {{ t('studio.chosenFiles', { count: entries.length }) }}
          </p>
        </div>
        <div v-else class="flex flex-col gap-1.5">
          <Label for="nginx-text">{{ t('studio.pasteNginx') }}</Label>
          <Textarea
            id="nginx-text"
            v-model="pasted"
            rows="10"
            spellcheck="false"
            class="font-mono text-xs"
            placeholder="server {&#10;    listen 80;&#10;    server_name example.com;&#10;}"
          />
        </div>

        <template v-if="result">
          <Alert v-if="!result.valid">
            <TriangleAlert aria-hidden="true" />
            <AlertTitle>{{ t('studio.importInvalid') }}</AlertTitle>
            <AlertDescription>
              <DiagnosticList :diagnostics="result.diagnostics" />
            </AlertDescription>
          </Alert>
          <section class="flex flex-col gap-2">
            <h3 class="text-sm font-medium">
              {{ t('studio.importReport', { count: result.report.length }) }}
            </h3>
            <DiagnosticList v-if="result.report.length" :diagnostics="result.report" />
            <p v-else class="text-muted-foreground text-sm">{{ t('studio.importComplete') }}</p>
          </section>
          <section class="flex flex-col gap-2">
            <h3 class="text-sm font-medium">{{ t('studio.importResult') }}</h3>
            <div class="h-80 overflow-hidden rounded-md border">
              <CodeEditor
                :model-value="result.files['main.conf'] ?? ''"
                path="main.conf"
                :label="t('studio.editorLabel', { path: 'main.conf' })"
                readonly
              />
            </div>
          </section>
        </template>
      </div>

      <SheetFooter class="flex-row flex-wrap justify-end gap-2">
        <Button
          variant="outline"
          :disabled="convert.isPending.value || !(files[entry] ?? '').trim()"
          @click="run"
        >
          <Spinner v-if="convert.isPending.value" data-icon="inline-start" />
          <RefreshCw v-else data-icon="inline-start" aria-hidden="true" />
          {{ t('studio.convert') }}
        </Button>
        <Button :disabled="!result?.valid" @click="use">
          <FileInput data-icon="inline-start" aria-hidden="true" />
          {{ t('studio.useAsDraft') }}
        </Button>
      </SheetFooter>
    </SheetContent>
  </Sheet>
</template>
