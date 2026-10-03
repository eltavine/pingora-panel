<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Eye, Inbox, Siren } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { ApprovalRequest, AuditEvent } from '@/api/generated'
import {
  approveRequestMutation,
  listApprovalRequestsOptions,
  listAuditEventsOptions,
  rejectRequestMutation,
  revokeApprovalMutation,
  withdrawRequestMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
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
import ApprovalRequestSheet from './ApprovalRequestSheet.vue'
import { type Decision, stateVariant, validApprovals } from './presentation'

const { t, d } = useI18n()
const { can } = useSession()
const requests = useQuery(listApprovalRequestsOptions({ query: { limit: 50 } }))
const bypasses = useQuery({
  ...listAuditEventsOptions({ query: { type: 'config.approval.bypassed', limit: 5 } }),
  enabled: computed(() => can('audit.read')),
})
const mutations = {
  approve: useMutation(approveRequestMutation()),
  reject: useMutation(rejectRequestMutation()),
  revoke: useMutation(revokeApprovalMutation()),
  withdraw: useMutation(withdrawRequestMutation()),
}
const busy = computed(() => Object.values(mutations).some((mutation) => mutation.isPending.value))

const selected = ref<ApprovalRequest>()
const sheetOpen = ref(false)
function show(request: ApprovalRequest) {
  selected.value = request
  sheetOpen.value = true
}

function decide(decision: Decision, reason?: string) {
  const request = selected.value
  if (!request) {
    return
  }
  const options = {
    onSuccess: (updated: ApprovalRequest) => {
      toast.success(t(`approvals.done.${decision}`))
      selected.value = updated
      void requests.refetch()
    },
    onError: (error: unknown) => notifyFailure(error, t('common.changeFailed')),
  }
  const target = { path: { id: request.id }, headers: plainHeaders() }
  if (decision === 'reject') {
    mutations.reject.mutate({ ...target, body: { reason } }, options)
  } else {
    mutations[decision].mutate(target, options)
  }
}

/** Why and for which incident approvals were bypassed. */
function bypass(event: AuditEvent): { reason?: string; incident?: string } {
  return event.data as { reason?: string; incident?: string }
}
</script>

<template>
  <div class="flex flex-col gap-4">
    <Alert v-if="bypasses.data.value?.items.length" role="status">
      <Siren aria-hidden="true" />
      <AlertTitle>{{ t('approvals.bypasses') }}</AlertTitle>
      <AlertDescription>
        <ul class="flex flex-col gap-0.5">
          <li v-for="event in bypasses.data.value.items" :key="event.sequence">
            <span class="font-medium">{{ event.actor_id }}</span>
            <template v-if="event.occurred_at">
              · {{ d(new Date(event.occurred_at), 'datetime') }}</template
            >
            · {{ bypass(event).incident }} · {{ bypass(event).reason }}
          </li>
        </ul>
      </AlertDescription>
    </Alert>

    <ApiFailureAlert
      v-if="requests.isError.value && !requests.data.value"
      :error="requests.error.value"
      retryable
      @retry="requests.refetch()"
    />
    <div v-else-if="requests.isPending.value" class="flex flex-col gap-2">
      <Skeleton v-for="index in 3" :key="index" class="h-12 w-full" />
    </div>
    <div
      v-else-if="!requests.data.value?.items.length"
      class="text-muted-foreground flex flex-col items-center gap-2 rounded-lg border border-dashed p-10 text-center text-sm"
    >
      <Inbox class="size-6" aria-hidden="true" />
      {{ t('approvals.empty') }}
    </div>
    <div v-else class="rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{{ t('approvals.request') }}</TableHead>
            <TableHead>{{ t('approvals.approvals') }}</TableHead>
            <TableHead class="w-12"
              ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="request in requests.data.value.items" :key="request.id">
            <TableCell class="align-top whitespace-normal">
              <span class="flex flex-wrap items-center gap-2">
                <Badge :variant="stateVariant(request.state)">{{
                  t(`approvals.states.${request.state}`)
                }}</Badge>
                <span class="font-medium">{{ request.requested_by }}</span>
                <Badge v-if="request.risk === 'high'" variant="outline">{{
                  t('approvals.risks.high')
                }}</Badge>
              </span>
              <span class="text-muted-foreground block text-xs tabular-nums">
                {{ d(new Date(request.requested_at), 'datetime') }} ·
                {{ t('approvals.changeCount', { count: request.changes.length }) }} ·
                {{ request.policies.map((policy) => policy.id).join(', ') }}
              </span>
            </TableCell>
            <TableCell class="align-top tabular-nums">
              {{ validApprovals(request) }}/{{ request.required }}
            </TableCell>
            <TableCell class="align-top">
              <Button
                variant="ghost"
                size="icon-sm"
                :aria-label="t('approvals.open')"
                :title="t('approvals.open')"
                @click="show(request)"
              >
                <Eye aria-hidden="true" />
              </Button>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <ApprovalRequestSheet
      v-model:open="sheetOpen"
      :request="selected"
      :busy="busy"
      @decide="decide"
    />
  </div>
</template>
