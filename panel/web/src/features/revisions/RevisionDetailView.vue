<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import {
  ArrowLeft,
  Check,
  FileCode2,
  GitCompareArrows,
  History,
  Pencil,
  RotateCcw,
  ShieldCheck,
  Undo2,
  X,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { toast } from 'vue-sonner'
import type { DiagnosticDetails } from '@/api/generated'
import {
  applyMutation,
  diffRevisionOptions,
  getRevisionOptions,
  getRevisionQueryKey,
  noteRevisionMutation,
  restoreRevisionMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ChangeSet from '@/components/ChangeSet.vue'
import CodeEditor from '@/components/code/CodeEditor.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import CopyValue from '@/components/CopyValue.vue'
import DiagnosticList from '@/components/DiagnosticList.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Textarea } from '@/components/ui/textarea'
import { isApprovalRequest } from '@/lib/approvals'
import { notifyFailure, plainHeaders, useRefreshConfiguration } from '@/lib/configuration'
import { sortPaths } from '@/features/config-studio/useConfigFiles'
import { isComparison, outcomeTones, type Comparison } from './presentation'

const TABS = ['changes', 'files'] as const
type Tab = (typeof TABS)[number]

const props = defineProps<{ id: string }>()

const { t, d } = useI18n()
const route = useRoute()
const router = useRouter()
const client = useQueryClient()
const refresh = useRefreshConfiguration()
const path = computed(() => ({ id: Number(props.id) }))

const tab = computed<Tab>({
  get: () => TABS.find((item) => item === route.query.tab) ?? 'changes',
  set: (value) => void router.replace({ query: { ...route.query, tab: value } }),
})
const against = computed<Comparison>({
  get: () => (isComparison(route.query.against) ? route.query.against : 'previous'),
  set: (value) => void router.replace({ query: { ...route.query, against: value } }),
})

const detail = useQuery(computed(() => getRevisionOptions({ path: path.value })))
const diff = useQuery(
  computed(() => ({
    ...diffRevisionOptions({ path: path.value, query: { against: against.value } }),
    enabled: tab.value === 'changes',
  })),
)
const revision = computed(() => detail.data.value?.revision)
const files = computed(() => sortPaths(Object.keys(detail.data.value?.files ?? {})))
const file = ref('main.conf')
watch(files, (paths) => {
  if (!paths.includes(file.value) && paths[0]) {
    file.value = paths[0]
  }
})
const diagnostics = computed(
  () => (revision.value?.diagnostics ?? []) as unknown as DiagnosticDetails[],
)

const facts = computed(() => {
  const value = revision.value
  if (!value) {
    return []
  }
  return [
    { label: t('revisions.columns.author'), value: value.author },
    { label: t('revisions.columns.created'), value: d(new Date(value.created_at), 'datetime') },
    {
      label: t('revisions.settledAt'),
      value: value.outcome_at ? d(new Date(value.outcome_at), 'datetime') : t('state.none'),
    },
    { label: t('revisions.draftVersion'), value: `v${value.draft_version}` },
    { label: t('revisions.languageVersion'), value: String(value.language_version) },
    {
      label: t('revisions.gatewayRevision'),
      value: value.gateway_revision ? `#${value.gateway_revision}` : t('state.none'),
    },
  ]
})

const noting = ref(false)
const noteText = ref('')
const note = useMutation(noteRevisionMutation())
function editNote() {
  noteText.value = revision.value?.note ?? ''
  noting.value = true
}
function saveNote() {
  note.mutate(
    { path: path.value, body: { note: noteText.value.trim() }, headers: plainHeaders() },
    {
      onSuccess: (updated) => {
        const key = getRevisionQueryKey({ path: path.value })
        const current = detail.data.value
        if (current) {
          client.setQueryData(key, { ...current, revision: updated })
        }
        noting.value = false
        toast.success(t('revisions.noteSaved'))
        void client.invalidateQueries({ queryKey: [{ _id: 'listRevisions' }] })
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

const restore = useMutation(restoreRevisionMutation())
const apply = useMutation(applyMutation())
const restoring = ref(false)
const rollingBack = ref(false)
const reason = ref('')

function confirmRestore() {
  restore.mutate(
    { path: path.value, headers: plainHeaders() },
    {
      onSuccess: () => {
        restoring.value = false
        toast.success(t('revisions.restored', { id: props.id }))
        void refresh()
        void router.push('/config')
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

function confirmRollback() {
  restore.mutate(
    { path: path.value, headers: plainHeaders() },
    {
      onSuccess: (source) =>
        apply.mutate(
          {
            body: {
              expected_version: source.version,
              note: reason.value.trim() || t('revisions.rollbackNote', { id: props.id }),
            },
            headers: plainHeaders(),
          },
          {
            onSuccess: (result) => {
              rollingBack.value = false
              reason.value = ''
              void refresh()
              if (isApprovalRequest(result)) {
                toast.info(t('studio.waitingTitle'))
                void router.push('/approvals')
                return
              }
              toast.success(t('revisions.rolledBack', { revision: result.revision }))
              void router.push(`/revisions/${result.revision}`)
            },
            onError: (error) => notifyFailure(error, t('draft.applyFailed')),
          },
        ),
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <div>
      <Button variant="ghost" size="sm" as-child>
        <RouterLink to="/revisions">
          <ArrowLeft data-icon="inline-start" aria-hidden="true" />
          {{ t('common.back') }}
        </RouterLink>
      </Button>
    </div>

    <ApiFailureAlert
      v-if="detail.isError.value && !detail.data.value"
      :error="detail.error.value"
      retryable
      @retry="detail.refetch()"
    />
    <Skeleton v-else-if="!revision" class="h-40 w-full" />

    <template v-else>
      <PageHeader :icon="History" :title="t('revisions.detail', { id: revision.id })">
        <template #actions>
          <Button variant="outline" size="sm" @click="restoring = true">
            <RotateCcw data-icon="inline-start" aria-hidden="true" />
            {{ t('revisions.restore') }}
          </Button>
          <Button size="sm" :disabled="revision.outcome === 'active'" @click="rollingBack = true">
            <Undo2 data-icon="inline-start" aria-hidden="true" />
            {{ t('revisions.rollback') }}
          </Button>
        </template>
      </PageHeader>

      <Card>
        <CardHeader class="flex flex-row flex-wrap items-center gap-3">
          <StatusIndicator
            :tone="outcomeTones[revision.outcome]"
            :label="t(`revisions.outcome.${revision.outcome}`)"
          />
          <CardTitle class="sr-only">{{ t('revisions.detail', { id: revision.id }) }}</CardTitle>
        </CardHeader>
        <CardContent class="flex flex-col gap-5">
          <div class="flex items-start gap-2">
            <form
              v-if="noting"
              class="flex w-full max-w-xl items-center gap-1"
              @submit.prevent="saveNote"
            >
              <Input v-model="noteText" maxlength="1000" :aria-label="t('common.note')" autofocus />
              <Button
                type="submit"
                variant="ghost"
                size="icon-sm"
                :disabled="note.isPending.value"
                :aria-label="t('common.save')"
              >
                <Check aria-hidden="true" />
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                :aria-label="t('common.cancel')"
                @click="noting = false"
              >
                <X aria-hidden="true" />
              </Button>
            </form>
            <template v-else>
              <p :class="{ 'text-muted-foreground': !revision.note }">
                {{ revision.note ?? t('revisions.noNote') }}
              </p>
              <Button
                variant="ghost"
                size="icon-xs"
                :aria-label="t('revisions.editNote')"
                @click="editNote"
              >
                <Pencil aria-hidden="true" />
              </Button>
            </template>
          </div>
          <dl class="grid gap-x-6 gap-y-3 text-sm sm:grid-cols-2 lg:grid-cols-3">
            <div v-for="fact in facts" :key="fact.label" class="flex flex-col gap-0.5">
              <dt class="text-muted-foreground text-xs">{{ fact.label }}</dt>
              <dd class="tabular-nums">{{ fact.value }}</dd>
            </div>
            <div class="flex min-w-0 flex-col gap-0.5">
              <dt class="text-muted-foreground text-xs">{{ t('revisions.contentHash') }}</dt>
              <dd class="min-w-0"><CopyValue :value="revision.content_hash" /></dd>
            </div>
            <div v-if="revision.snapshot_hash" class="flex min-w-0 flex-col gap-0.5">
              <dt class="text-muted-foreground text-xs">{{ t('revisions.snapshotHash') }}</dt>
              <dd class="min-w-0"><CopyValue :value="revision.snapshot_hash" /></dd>
            </div>
          </dl>
          <section v-if="diagnostics.length" class="flex flex-col gap-2 rounded-md border p-3">
            <h2 class="text-sm font-medium">{{ t('revisions.diagnostics') }}</h2>
            <DiagnosticList :diagnostics="diagnostics" />
          </section>
        </CardContent>
      </Card>

      <Tabs v-model="tab">
        <TabsList>
          <TabsTrigger value="changes">
            <GitCompareArrows aria-hidden="true" />{{ t('revisions.tabs.changes') }}
          </TabsTrigger>
          <TabsTrigger value="files">
            <FileCode2 aria-hidden="true" />{{ t('revisions.tabs.files') }}
          </TabsTrigger>
        </TabsList>

        <TabsContent value="changes" class="flex flex-col gap-4 pt-2">
          <div class="flex items-center gap-2">
            <Label for="revision-against">{{ t('revisions.against') }}</Label>
            <Select v-model="against">
              <SelectTrigger id="revision-against" class="w-48">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="previous">{{ t('revisions.againstPrevious') }}</SelectItem>
                <SelectItem value="active">{{ t('revisions.againstActive') }}</SelectItem>
                <SelectItem value="draft">{{ t('revisions.againstDraft') }}</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <ApiFailureAlert v-if="diff.isError.value" :error="diff.error.value" />
          <Skeleton v-else-if="!diff.data.value" class="h-32 w-full" />
          <p
            v-else-if="!diff.data.value.resources.length && !diff.data.value.files.length"
            class="text-muted-foreground flex items-center gap-2 rounded-md border p-4 text-sm"
          >
            <ShieldCheck class="size-4" aria-hidden="true" />
            {{ t('revisions.identical') }}
          </p>
          <ChangeSet v-else :changes="diff.data.value" />
        </TabsContent>

        <TabsContent value="files" class="pt-2">
          <div class="grid gap-4 lg:grid-cols-[15rem_minmax(0,1fr)]">
            <ul class="flex flex-col gap-0.5" :aria-label="t('studio.files')">
              <li v-for="name in files" :key="name">
                <button
                  type="button"
                  class="hover:bg-muted flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left"
                  :class="{ 'bg-muted font-medium': name === file }"
                  :aria-current="name === file ? 'true' : undefined"
                  @click="file = name"
                >
                  <FileCode2 class="size-4 shrink-0" aria-hidden="true" />
                  <span class="truncate font-mono text-xs">{{ name }}</span>
                </button>
              </li>
            </ul>
            <div class="h-[min(60vh,40rem)] min-h-64 overflow-hidden rounded-lg border">
              <CodeEditor
                :model-value="detail.data.value?.files[file] ?? ''"
                :path="file"
                :label="t('studio.editorLabel', { path: file })"
                readonly
              />
            </div>
          </div>
        </TabsContent>
      </Tabs>
    </template>

    <ConfirmDialog
      v-model:open="restoring"
      :icon="RotateCcw"
      :title="t('revisions.restoreTitle', { id })"
      :description="t('revisions.restoreDetail')"
      :confirm-label="t('revisions.restore')"
      :busy="restore.isPending.value"
      @confirm="confirmRestore"
    />
    <ConfirmDialog
      v-model:open="rollingBack"
      :icon="Undo2"
      :title="t('revisions.rollbackTitle', { id })"
      :description="t('revisions.rollbackDetail')"
      :confirm-label="t('revisions.rollback')"
      :busy="restore.isPending.value || apply.isPending.value"
      @confirm="confirmRollback"
    >
      <div class="flex flex-col gap-1.5">
        <Label for="rollback-reason">{{ t('revisions.rollbackReason') }}</Label>
        <Textarea
          id="rollback-reason"
          v-model="reason"
          rows="2"
          maxlength="1000"
          :placeholder="t('revisions.rollbackNote', { id })"
        />
      </div>
    </ConfirmDialog>
  </div>
</template>
