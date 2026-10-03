<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Pencil, Plus, ShieldCheck, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { ApprovalPolicy } from '@/api/generated'
import {
  deleteApprovalPolicyMutation,
  listApprovalPoliciesOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
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
import { useSession } from '@/lib/session'
import ApprovalPolicyFormSheet from './ApprovalPolicyFormSheet.vue'

const { t } = useI18n()
const { can } = useSession()
const canManage = computed(() => can('approval.manage'))
const policies = useQuery(listApprovalPoliciesOptions())
const remove = useMutation(deleteApprovalPolicyMutation())

const editing = ref<ApprovalPolicy>()
const formOpen = ref(false)
function openForm(policy?: ApprovalPolicy) {
  editing.value = policy
  formOpen.value = true
}

const removing = ref<ApprovalPolicy | null>(null)
const removeOpen = computed({
  get: () => removing.value !== null,
  set: (open) => {
    if (!open) {
      removing.value = null
    }
  },
})
function confirmRemove() {
  const policy = removing.value
  if (!policy) {
    return
  }
  remove.mutate(
    { path: { id: policy.id }, headers: plainHeaders() },
    {
      onSuccess: () => {
        toast.success(t('approvals.policyDeleted'))
        removing.value = null
        void policies.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

/** What a policy covers, in words. */
function covers(policy: ApprovalPolicy): string[] {
  const parts = [
    policy.resources?.length
      ? policy.resources.map((kind) => t(`approvals.kinds.${kind}`)).join(', ')
      : t('approvals.anyChange'),
  ]
  if (policy.site_tags?.length) {
    parts.push(t('approvals.taggedSites', { tags: policy.site_tags.join(', ') }))
  }
  if (policy.min_risk === 'high') {
    parts.push(t('approvals.highRiskOnly'))
  }
  for (const window of policy.windows ?? []) {
    const days = (window.days ?? []).map((day) => t(`approvals.days.${day}`)).join(' ')
    parts.push(`${days ? `${days} ` : ''}${window.start}–${window.end} UTC`)
  }
  return parts
}
</script>

<template>
  <div class="flex flex-col gap-4">
    <div v-if="canManage" class="flex justify-end">
      <Button size="sm" @click="openForm()">
        <Plus data-icon="inline-start" aria-hidden="true" />
        {{ t('approvals.newPolicy') }}
      </Button>
    </div>
    <ApiFailureAlert
      v-if="policies.isError.value && !policies.data.value"
      :error="policies.error.value"
      retryable
      @retry="policies.refetch()"
    />
    <div v-else-if="policies.isPending.value" class="flex flex-col gap-2">
      <Skeleton v-for="index in 2" :key="index" class="h-12 w-full" />
    </div>
    <div
      v-else-if="!policies.data.value?.length"
      class="text-muted-foreground flex flex-col items-center gap-2 rounded-lg border border-dashed p-10 text-center text-sm"
    >
      <ShieldCheck class="size-6" aria-hidden="true" />
      {{ t('approvals.noPolicies') }}
    </div>
    <div v-else class="rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{{ t('approvals.policy') }}</TableHead>
            <TableHead>{{ t('approvals.required') }}</TableHead>
            <TableHead class="w-20"
              ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="policy in policies.data.value" :key="policy.id">
            <TableCell class="align-top whitespace-normal">
              <span class="flex flex-wrap items-center gap-2 font-medium">
                <code class="font-mono">{{ policy.id }}</code>
                <Badge variant="outline">v{{ policy.version }}</Badge>
                <Badge v-if="policy.enabled === false" variant="outline">{{
                  t('approvals.disabled')
                }}</Badge>
              </span>
              <span v-if="policy.description" class="text-muted-foreground block text-xs">
                {{ policy.description }}
              </span>
              <span class="mt-1 flex flex-wrap gap-1">
                <Badge v-for="part in covers(policy)" :key="part" variant="secondary">{{
                  part
                }}</Badge>
              </span>
            </TableCell>
            <TableCell class="align-top text-sm tabular-nums">
              {{
                t('approvals.requiredDetail', {
                  count: policy.approvals ?? 1,
                  minutes: policy.valid_minutes ?? 60,
                })
              }}
            </TableCell>
            <TableCell class="align-top">
              <span
                v-if="canManage"
                class="flex flex-col items-end gap-1 sm:flex-row sm:justify-end"
              >
                <Button
                  variant="ghost"
                  size="icon-sm"
                  :aria-label="t('common.edit')"
                  :title="t('common.edit')"
                  @click="openForm(policy)"
                >
                  <Pencil aria-hidden="true" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  :aria-label="t('common.delete')"
                  :title="t('common.delete')"
                  @click="removing = policy"
                >
                  <Trash2 aria-hidden="true" />
                </Button>
              </span>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <ApprovalPolicyFormSheet
      v-model:open="formOpen"
      :policy="editing"
      @saved="policies.refetch()"
    />
    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="t('approvals.deletePolicyTitle', { id: removing?.id ?? '' })"
      :description="t('approvals.deletePolicyDetail')"
      :confirm-label="t('common.delete')"
      destructive
      :busy="remove.isPending.value"
      @confirm="confirmRemove"
    />
  </div>
</template>
