<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import {
  CloudUpload,
  FileDiff,
  FlaskConical,
  GitCompareArrows,
  Minus,
  PencilLine,
  Plus,
  RefreshCw,
  ShieldCheck,
  Siren,
  Stamp,
  TriangleAlert,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { ApprovalRequest, DiagnosticDetails } from '@/api/generated'
import { applyMutation, dryRunMutation, planOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ChangeSet from '@/components/ChangeSet.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import DiagnosticList from '@/components/DiagnosticList.vue'
import FormField from '@/components/FormField.vue'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
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
import { isApprovalRequest } from '@/lib/approvals'
import { notifyFailure, plainHeaders, useRefreshApplied } from '@/lib/configuration'
import { useSession } from '@/lib/session'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{
  /** The saved draft version to check and apply. */
  version?: number
  /** Unsaved edits exist; the plan covers only the saved draft. */
  dirty: boolean
}>()

const { t } = useI18n()
const { can } = useSession()
const refresh = useRefreshApplied()
const plan = useQuery(computed(() => ({ ...planOptions(), enabled: open.value })))
const dryRun = useMutation(dryRunMutation())
const apply = useMutation(applyMutation())
const note = ref('')
const problems = ref<DiagnosticDetails[]>([])
const passed = ref(false)
/** The approval request applying waits on. */
const waiting = ref<ApprovalRequest>()
const bypassReason = ref('')
const incident = ref('')
const bypassReady = computed(
  () => bypassReason.value.trim().length >= 10 && incident.value.trim().length > 0,
)

const empty = computed(
  () => plan.data.value && !plan.data.value.resources.length && !plan.data.value.files.length,
)
/** The draft version the plan reads, which applying names. */
const planned = computed(() => plan.data.value?.draft_version ?? props.version ?? 0)
const confirming = ref(false)
/** Applying found another plan than the one shown, which is shown anew. */
const changed = ref(false)
const counts = computed(() => {
  const resources = plan.data.value?.resources ?? []
  return {
    added: resources.filter((item) => item.change === 'added').length,
    changed: resources.filter((item) => item.change === 'changed').length,
    removed: resources.filter((item) => item.change === 'removed').length,
    files: plan.data.value?.files.length ?? 0,
  }
})

watch(open, (value) => {
  if (value) {
    problems.value = []
    passed.value = false
    waiting.value = undefined
    changed.value = false
    bypassReason.value = ''
    incident.value = ''
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

function runApply(bypass = false) {
  changed.value = false
  apply.mutate(
    {
      body: {
        expected_version: planned.value,
        expected_plan: plan.data.value?.digest,
        note: note.value.trim() || undefined,
        bypass: bypass
          ? { reason: bypassReason.value.trim(), incident: incident.value.trim() }
          : undefined,
      },
      headers: plainHeaders(),
    },
    {
      onSuccess: (result) => {
        if (isApprovalRequest(result)) {
          waiting.value = result
          toast.info(t('studio.waitingTitle'))
          return
        }
        toast.success(t('studio.applied', { revision: result.revision }))
        note.value = ''
        open.value = false
        void refresh()
      },
      onError: (error) => {
        const failure = toApiFailure(error)
        if (failure.kind === 'problem' && failure.problem.status === 409) {
          changed.value = true
          void plan.refetch()
          return
        }
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

        <Alert v-if="changed" role="alert">
          <RefreshCw aria-hidden="true" />
          <AlertTitle>{{ t('studio.planChanged') }}</AlertTitle>
          <AlertDescription>{{ t('studio.planChangedDetail') }}</AlertDescription>
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

        <Alert v-if="waiting" role="status">
          <Stamp aria-hidden="true" />
          <AlertTitle>{{ t('studio.waitingTitle') }}</AlertTitle>
          <AlertDescription>
            {{
              t('studio.waitingDetail', {
                count: waiting.required,
                policies: waiting.policies.map((policy) => policy.id).join(', '),
              })
            }}
            <RouterLink to="/approvals" class="underline">{{
              t('studio.openApprovals')
            }}</RouterLink>
          </AlertDescription>
        </Alert>
        <fieldset
          v-if="waiting && can('approval.bypass')"
          class="flex flex-col gap-3 rounded-md border border-dashed p-3"
        >
          <legend class="flex items-center gap-2 px-1 text-sm font-medium">
            <Siren class="size-4" aria-hidden="true" />
            {{ t('studio.bypass') }}
          </legend>
          <p class="text-muted-foreground text-xs">{{ t('studio.bypassHint') }}</p>
          <FormField id="bypass-reason" :label="t('studio.bypassReason')">
            <Textarea id="bypass-reason" v-model="bypassReason" rows="2" maxlength="512" />
          </FormField>
          <FormField id="bypass-incident" :label="t('studio.incident')">
            <Input id="bypass-incident" v-model="incident" maxlength="128" />
          </FormField>
          <Button
            variant="destructive"
            class="self-end"
            :disabled="apply.isPending.value || !bypassReady"
            @click="runApply(true)"
          >
            <Siren data-icon="inline-start" aria-hidden="true" />
            {{ t('studio.applyBypass') }}
          </Button>
        </fieldset>

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
        <Button
          :disabled="apply.isPending.value || empty || !plan.data.value"
          @click="confirming = true"
        >
          <Spinner v-if="apply.isPending.value" data-icon="inline-start" />
          <CloudUpload v-else data-icon="inline-start" aria-hidden="true" />
          {{ t('studio.apply', { version: planned }) }}
        </Button>
      </SheetFooter>
    </SheetContent>
  </Sheet>

  <ConfirmDialog
    v-model:open="confirming"
    :icon="CloudUpload"
    :title="t('studio.confirmTitle', { version: planned })"
    :description="
      plan.data.value?.active_revision == null
        ? t('studio.confirmFirst')
        : t('studio.confirmAgainst', { revision: plan.data.value.active_revision })
    "
    :confirm-label="t('studio.apply', { version: planned })"
    :busy="apply.isPending.value"
    @confirm="runApply()"
  >
    <ul class="flex flex-wrap gap-2 text-sm" :aria-label="t('studio.confirmSummary')">
      <li v-if="counts.added" class="flex items-center gap-1.5 rounded-md border px-2 py-1">
        <Plus class="size-4" aria-hidden="true" />{{
          t('studio.confirmAdded', { count: counts.added })
        }}
      </li>
      <li v-if="counts.changed" class="flex items-center gap-1.5 rounded-md border px-2 py-1">
        <PencilLine class="size-4" aria-hidden="true" />{{
          t('studio.confirmChanged', { count: counts.changed })
        }}
      </li>
      <li v-if="counts.removed" class="flex items-center gap-1.5 rounded-md border px-2 py-1">
        <Minus class="size-4" aria-hidden="true" />{{
          t('studio.confirmRemoved', { count: counts.removed })
        }}
      </li>
      <li v-if="counts.files" class="flex items-center gap-1.5 rounded-md border px-2 py-1">
        <FileDiff class="size-4" aria-hidden="true" />{{ t('studio.confirmFiles', counts.files) }}
      </li>
    </ul>
    <p class="text-muted-foreground font-mono text-xs break-all">
      {{ t('studio.confirmPlan', { digest: plan.data.value?.digest ?? '' }) }}
    </p>
  </ConfirmDialog>
</template>
