<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import {
  CloudUpload,
  FlaskConical,
  GitCompareArrows,
  ShieldCheck,
  TriangleAlert,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { DiagnosticDetails } from '@/api/generated'
import { applyMutation, dryRunMutation, planOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ChangeSet from '@/components/ChangeSet.vue'
import DiagnosticList from '@/components/DiagnosticList.vue'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { Spinner } from '@/components/ui/spinner'
import { Textarea } from '@/components/ui/textarea'
import { toApiFailure } from '@/lib/api'
import { notifyFailure, plainHeaders, useRefreshConfiguration } from '@/lib/configuration'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{
  /** The saved draft version to check and apply. */
  version?: number
  /** Unsaved edits exist; the plan covers only the saved draft. */
  dirty: boolean
}>()

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const plan = useQuery(computed(() => ({ ...planOptions(), enabled: open.value })))
const dryRun = useMutation(dryRunMutation())
const apply = useMutation(applyMutation())
const note = ref('')
const problems = ref<DiagnosticDetails[]>([])
const passed = ref(false)

const empty = computed(
  () => plan.data.value && !plan.data.value.resources.length && !plan.data.value.files.length,
)

watch(open, (value) => {
  if (value) {
    problems.value = []
    passed.value = false
    void plan.refetch()
  }
})

function rejected(error: unknown): boolean {
  const failure = toApiFailure(error)
  if (failure.kind === 'problem' && failure.problem.field_errors?.length) {
    problems.value = failure.problem.field_errors
    return true
  }
  return false
}

function runDryRun() {
  problems.value = []
  passed.value = false
  dryRun.mutate(
    { body: { expected_version: props.version }, headers: plainHeaders() },
    {
      onSuccess: (result) => {
        passed.value = true
        problems.value = result.diagnostics
        toast.success(t('studio.dryRunPassed', { version: result.draft.version }))
      },
      onError: (error) => {
        if (!rejected(error)) {
          notifyFailure(error, t('studio.dryRunFailed'))
        }
      },
    },
  )
}

function runApply() {
  apply.mutate(
    {
      body: { expected_version: props.version, note: note.value.trim() || undefined },
      headers: plainHeaders(),
    },
    {
      onSuccess: (result) => {
        toast.success(t('studio.applied', { revision: result.revision }))
        note.value = ''
        open.value = false
        void refresh()
      },
      onError: (error) => {
        if (!rejected(error)) {
          notifyFailure(error, t('draft.applyFailed'))
        }
      },
    },
  )
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-3xl">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2">
          <GitCompareArrows class="size-5" aria-hidden="true" />
          {{ t('studio.plan') }}
        </SheetTitle>
        <SheetDescription>{{ t('studio.planDetail', { version: version ?? 0 }) }}</SheetDescription>
      </SheetHeader>

      <div class="flex flex-col gap-4 px-4">
        <Alert v-if="dirty">
          <TriangleAlert aria-hidden="true" />
          <AlertTitle>{{ t('studio.unsaved') }}</AlertTitle>
          <AlertDescription>{{ t('studio.planUnsaved') }}</AlertDescription>
        </Alert>

        <ApiFailureAlert v-if="plan.isError.value" :error="plan.error.value" />
        <div v-else-if="plan.isPending.value" class="flex flex-col gap-2">
          <Skeleton v-for="index in 3" :key="index" class="h-10 w-full" />
        </div>
        <p
          v-else-if="empty"
          class="text-muted-foreground flex items-center gap-2 rounded-md border p-4 text-sm"
        >
          <ShieldCheck class="size-4" aria-hidden="true" />
          {{ t('studio.noChanges') }}
        </p>
        <ChangeSet v-else-if="plan.data.value" :changes="plan.data.value" />

        <section v-if="problems.length" class="flex flex-col gap-2 rounded-md border p-3">
          <h3 class="text-sm font-medium">{{ t('studio.problems') }}</h3>
          <DiagnosticList :diagnostics="problems" />
        </section>

        <div class="flex flex-col gap-1.5">
          <Label for="revision-note">{{ t('studio.note') }}</Label>
          <Textarea
            id="revision-note"
            v-model="note"
            rows="2"
            maxlength="1000"
            :placeholder="t('studio.notePlaceholder')"
          />
        </div>
      </div>

      <SheetFooter class="flex-row flex-wrap justify-end gap-2">
        <Button variant="outline" :disabled="dryRun.isPending.value || empty" @click="runDryRun">
          <Spinner v-if="dryRun.isPending.value" data-icon="inline-start" />
          <ShieldCheck v-else-if="passed" data-icon="inline-start" aria-hidden="true" />
          <FlaskConical v-else data-icon="inline-start" aria-hidden="true" />
          {{ t('studio.dryRun') }}
        </Button>
        <Button :disabled="apply.isPending.value || empty" @click="runApply">
          <Spinner v-if="apply.isPending.value" data-icon="inline-start" />
          <CloudUpload v-else data-icon="inline-start" aria-hidden="true" />
          {{ t('studio.apply', { version: version ?? 0 }) }}
        </Button>
      </SheetFooter>
    </SheetContent>
  </Sheet>
</template>
