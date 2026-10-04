<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { CircleCheck, CloudUpload, FilePen, ShieldCheck, TriangleAlert } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import {
  applyMutation,
  draftOptions,
  validationOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { notifyFailure, plainHeaders, useRefreshApplied } from '@/lib/configuration'

const { t } = useI18n()
const refresh = useRefreshApplied()
const draft = useQuery({ ...draftOptions(), refetchInterval: 15_000 })
const confirming = ref(false)
const note = ref('')
const validation = useQuery(computed(() => ({ ...validationOptions(), enabled: confirming.value })))
const apply = useMutation(applyMutation())
const pending = computed(() => draft.data.value?.pending ?? false)
const diagnostics = computed(
  () => (validation.data.value?.diagnostics ?? []) as { resource_id?: string; message: string }[],
)

function confirm() {
  const version = draft.data.value?.version
  apply.mutate(
    {
      body: { expected_version: version, note: note.value.trim() || undefined },
      headers: plainHeaders(),
    },
    {
      onSuccess: (result) => {
        toast.success(t('draft.appliedRevision', { revision: result.revision }))
        confirming.value = false
        note.value = ''
        void refresh()
      },
      onError: (error) => notifyFailure(error, t('draft.applyFailed')),
    },
  )
}
</script>

<template>
  <div v-if="draft.data.value" class="flex items-center gap-2">
    <Badge v-if="pending" variant="outline" class="hidden gap-1 sm:inline-flex">
      <FilePen class="size-3.5" aria-hidden="true" />
      {{ t('draft.pending', { version: draft.data.value.version }) }}
    </Badge>
    <Badge v-else variant="secondary" class="hidden gap-1 sm:inline-flex">
      <CircleCheck class="size-3.5" aria-hidden="true" />
      {{ t('draft.applied', { version: draft.data.value.applied_version ?? 0 }) }}
    </Badge>
    <Button
      size="sm"
      :disabled="!pending"
      :aria-label="t('draft.apply')"
      :title="pending ? t('draft.pending', { version: draft.data.value.version }) : undefined"
      @click="confirming = true"
    >
      <CloudUpload data-icon="inline-start" aria-hidden="true" />
      <span class="hidden sm:inline">{{ t('draft.apply') }}</span>
    </Button>
    <AlertDialog v-model:open="confirming">
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle class="flex items-center gap-2">
            <CloudUpload class="size-5" aria-hidden="true" />
            {{ t('draft.confirmTitle', { version: draft.data.value.version }) }}
          </AlertDialogTitle>
          <AlertDialogDescription>{{ t('draft.confirmDetail') }}</AlertDialogDescription>
        </AlertDialogHeader>
        <div v-if="validation.data.value" class="flex flex-col gap-2 text-sm">
          <p v-if="validation.data.value.valid" class="flex items-center gap-2">
            <ShieldCheck class="size-4" aria-hidden="true" />
            {{ t('draft.valid') }}
          </p>
          <template v-else>
            <p class="flex items-center gap-2 font-medium">
              <TriangleAlert class="size-4" aria-hidden="true" />
              {{ t('draft.invalid', { count: diagnostics.length }) }}
            </p>
            <ul class="max-h-48 list-inside list-disc overflow-y-auto">
              <li v-for="item in diagnostics" :key="`${item.resource_id}-${item.message}`">
                <span class="font-mono text-xs">{{ item.resource_id }}</span> {{ item.message }}
              </li>
            </ul>
          </template>
        </div>
        <div class="flex flex-col gap-1.5">
          <Label for="apply-note">{{ t('draft.note') }}</Label>
          <Textarea
            id="apply-note"
            v-model="note"
            rows="2"
            maxlength="1000"
            :placeholder="t('draft.notePlaceholder')"
          />
        </div>
        <AlertDialogFooter>
          <AlertDialogCancel>{{ t('common.cancel') }}</AlertDialogCancel>
          <AlertDialogAction
            :disabled="apply.isPending.value || validation.data.value?.valid === false"
            @click.prevent="confirm"
          >
            {{ t('draft.apply') }}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  </div>
</template>
