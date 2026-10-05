<script setup lang="ts">
import { computed, ref, useTemplateRef } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import {
  Download,
  File as FileIcon,
  FileText,
  Folder,
  FolderOpen,
  FolderPlus,
  Link2,
  Pencil,
  RefreshCw,
  Trash2,
  Upload,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { toast } from 'vue-sonner'
import type { SiteEntryView } from '@/api/generated'
import { createDirectory, readFile, removeEntry, writeFile } from '@/api/generated'
import { listDirectoryOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import PageHeader from '@/components/PageHeader.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { downloadBlob } from '@/lib/download'
import { formatters } from '@/lib/format'
import { useSession } from '@/lib/session'
import FileEditorSheet from './FileEditorSheet.vue'
import { childPath, crumbs, editable, normalizedPath } from './presentation'

const { t, d, locale } = useI18n()
const format = computed(() => formatters(locale.value))
const route = useRoute()
const router = useRouter()
const { can } = useSession()
const writable = computed(() => can('files.write'))

const path = computed(() => normalizedPath(route.query.path))
const listing = useQuery(computed(() => listDirectoryOptions({ query: { path: path.value } })))
const entries = computed(() => listing.data.value?.entries ?? [])

function go(to: string) {
  void router.replace({ query: { ...route.query, path: to || undefined } })
}

const icons = { directory: Folder, file: FileIcon, link: Link2, other: FileIcon } as const
function iconOf(entry: SiteEntryView) {
  return entry.kind === 'file' && editable(entry.name, entry.size_bytes)
    ? FileText
    : icons[entry.kind]
}

const editing = ref('')
const editorOpen = ref(false)
function open(entry: SiteEntryView) {
  const target = childPath(path.value, entry.name)
  if (entry.kind === 'directory') {
    go(target)
  } else if (editable(entry.name, entry.size_bytes)) {
    editing.value = target
    editorOpen.value = true
  } else {
    void download(entry)
  }
}

async function download(entry: SiteEntryView) {
  try {
    const { data } = await readFile({
      query: { path: childPath(path.value, entry.name) },
      parseAs: 'blob',
      throwOnError: true,
    })
    downloadBlob(entry.name, data instanceof Blob ? data : new Blob([]))
  } catch (error) {
    notifyFailure(error, t('siteFiles.downloadFailed'))
  }
}

const picker = useTemplateRef<HTMLInputElement>('picker')
const uploading = ref(false)
async function upload(files: Iterable<File>) {
  const chosen = [...files]
  if (!chosen.length || !writable.value) {
    return
  }
  uploading.value = true
  let done = 0
  for (const file of chosen) {
    try {
      await writeFile({
        query: { path: childPath(path.value, file.name) },
        body: file,
        headers: plainHeaders(),
        throwOnError: true,
      })
      done += 1
    } catch (error) {
      notifyFailure(error, t('siteFiles.uploadFailed', { name: file.name }))
    }
  }
  uploading.value = false
  if (done) {
    toast.success(t('siteFiles.uploaded', { count: done }, done))
  }
  void listing.refetch()
}
function picked(event: Event) {
  const input = event.target as HTMLInputElement
  void upload(input.files ?? [])
  input.value = ''
}
const dragging = ref(false)
function dropped(event: DragEvent) {
  dragging.value = false
  void upload(event.dataTransfer?.files ?? [])
}

const naming = ref(false)
const folder = ref('')
async function createFolder() {
  const name = folder.value.trim()
  if (!name) {
    return
  }
  try {
    await createDirectory({
      query: { path: childPath(path.value, name) },
      headers: plainHeaders(),
      throwOnError: true,
    })
    toast.success(t('siteFiles.folderCreated', { name }))
    naming.value = false
    folder.value = ''
    void listing.refetch()
  } catch (error) {
    notifyFailure(error, t('common.changeFailed'))
  }
}

const removing = ref<SiteEntryView>()
const removeOpen = computed({
  get: () => removing.value !== undefined,
  set: (opened: boolean) => {
    if (!opened) {
      removing.value = undefined
    }
  },
})
const recursive = ref(false)
function askRemove(entry: SiteEntryView) {
  recursive.value = false
  removing.value = entry
}
async function remove() {
  const entry = removing.value
  if (!entry) {
    return
  }
  try {
    await removeEntry({
      query: { path: childPath(path.value, entry.name), recursive: recursive.value },
      headers: plainHeaders(),
      throwOnError: true,
    })
    toast.success(t('siteFiles.removed', { name: entry.name }))
    void listing.refetch()
  } catch (error) {
    notifyFailure(error, t('common.changeFailed'))
  }
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="FolderOpen"
      :title="t('siteFiles.title')"
      :description="t('siteFiles.description')"
    >
      <template #actions>
        <Button
          variant="outline"
          size="sm"
          :disabled="listing.isFetching.value"
          @click="listing.refetch()"
        >
          <RefreshCw
            data-icon="inline-start"
            :class="{ 'animate-spin': listing.isFetching.value }"
            aria-hidden="true"
          />
          {{ t('state.refresh') }}
        </Button>
        <template v-if="writable">
          <Button variant="outline" size="sm" @click="naming = true">
            <FolderPlus data-icon="inline-start" aria-hidden="true" />
            {{ t('siteFiles.newFolder') }}
          </Button>
          <Button size="sm" :disabled="uploading" @click="picker?.click()">
            <Upload data-icon="inline-start" aria-hidden="true" />
            {{ t('siteFiles.upload') }}
          </Button>
          <input
            ref="picker"
            type="file"
            multiple
            class="hidden"
            :aria-label="t('siteFiles.upload')"
            @change="picked"
          />
        </template>
      </template>
    </PageHeader>

    <Card
      :class="{ 'ring-ring ring-2': dragging }"
      @dragover.prevent="dragging = writable"
      @dragleave="dragging = false"
      @drop.prevent="dropped"
    >
      <CardContent class="flex flex-col gap-4">
        <nav
          :aria-label="t('siteFiles.location')"
          class="flex flex-wrap items-center gap-1 text-sm"
        >
          <Button variant="ghost" size="sm" class="h-7 px-2" @click="go('')">
            <FolderOpen data-icon="inline-start" aria-hidden="true" />
            {{ t('siteFiles.root') }}
          </Button>
          <template v-for="crumb in crumbs(path)" :key="crumb.path">
            <span class="text-muted-foreground" aria-hidden="true">/</span>
            <Button variant="ghost" size="sm" class="h-7 px-2 font-mono" @click="go(crumb.path)">
              {{ crumb.name }}
            </Button>
          </template>
        </nav>

        <form
          v-if="naming"
          class="flex flex-wrap items-center gap-2"
          @submit.prevent="createFolder"
        >
          <Input
            v-model="folder"
            class="max-w-64"
            autocomplete="off"
            :placeholder="t('siteFiles.folderName')"
            :aria-label="t('siteFiles.folderName')"
          />
          <Button type="submit" size="sm" :disabled="!folder.trim()">
            <FolderPlus data-icon="inline-start" aria-hidden="true" />
            {{ t('common.add') }}
          </Button>
          <Button type="button" variant="ghost" size="sm" @click="naming = false">
            {{ t('common.cancel') }}
          </Button>
        </form>

        <ApiFailureAlert
          v-if="listing.isError.value && !listing.data.value"
          :error="listing.error.value"
          retryable
          @retry="listing.refetch()"
        />
        <Skeleton
          v-else-if="listing.isPending.value"
          class="h-24 rounded-lg"
          aria-busy="true"
          :aria-label="t('state.loading')"
        />
        <div v-else-if="entries.length" class="overflow-x-auto">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{{ t('siteFiles.name') }}</TableHead>
                <TableHead>{{ t('siteFiles.size') }}</TableHead>
                <TableHead class="hidden sm:table-cell">{{ t('siteFiles.modified') }}</TableHead>
                <TableHead
                  ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
                >
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="entry in entries" :key="entry.name">
                <TableCell>
                  <Button
                    variant="link"
                    class="h-auto justify-start gap-2 p-0 text-left font-mono text-xs break-all whitespace-normal"
                    :disabled="entry.kind === 'link' || entry.kind === 'other'"
                    @click="open(entry)"
                  >
                    <component :is="iconOf(entry)" class="size-4 shrink-0" aria-hidden="true" />
                    {{ entry.name }}
                  </Button>
                </TableCell>
                <TableCell class="text-sm whitespace-nowrap tabular-nums">
                  {{ entry.kind === 'file' ? format.bytes(entry.size_bytes) : '—' }}
                </TableCell>
                <TableCell
                  class="text-muted-foreground hidden text-sm whitespace-nowrap sm:table-cell"
                >
                  {{ entry.modified ? d(new Date(entry.modified), 'datetime') : '—' }}
                </TableCell>
                <TableCell>
                  <div class="flex items-center justify-end gap-1">
                    <Button
                      v-if="entry.kind === 'file' && editable(entry.name, entry.size_bytes)"
                      variant="ghost"
                      size="icon-sm"
                      :aria-label="
                        writable
                          ? t('siteFiles.edit', { name: entry.name })
                          : t('siteFiles.view', { name: entry.name })
                      "
                      @click="open(entry)"
                    >
                      <Pencil v-if="writable" aria-hidden="true" />
                      <FileText v-else aria-hidden="true" />
                    </Button>
                    <Button
                      v-if="entry.kind === 'file'"
                      variant="ghost"
                      size="icon-sm"
                      :aria-label="t('siteFiles.download', { name: entry.name })"
                      @click="download(entry)"
                    >
                      <Download aria-hidden="true" />
                    </Button>
                    <Button
                      v-if="writable"
                      variant="ghost"
                      size="icon-sm"
                      :aria-label="t('siteFiles.remove', { name: entry.name })"
                      @click="askRemove(entry)"
                    >
                      <Trash2 aria-hidden="true" />
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
        <Empty v-else class="border border-dashed">
          <EmptyHeader>
            <EmptyMedia variant="icon"><Folder aria-hidden="true" /></EmptyMedia>
            <EmptyTitle>{{ t('siteFiles.empty') }}</EmptyTitle>
            <EmptyDescription v-if="writable">{{ t('siteFiles.emptyDetail') }}</EmptyDescription>
          </EmptyHeader>
        </Empty>
      </CardContent>
    </Card>

    <FileEditorSheet
      v-if="editing"
      v-model:open="editorOpen"
      :path="editing"
      :writable="writable"
      @saved="listing.refetch()"
    />
    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="t('siteFiles.removeTitle', { name: removing?.name ?? '' })"
      :description="t('siteFiles.removeDetail')"
      :confirm-label="t('common.delete')"
      destructive
      @confirm="remove"
    >
      <label v-if="removing?.kind === 'directory'" class="flex items-start gap-2 text-sm">
        <Checkbox v-model="recursive" class="mt-0.5" />
        <span>{{ t('siteFiles.recursive') }}</span>
      </label>
    </ConfirmDialog>
  </div>
</template>
