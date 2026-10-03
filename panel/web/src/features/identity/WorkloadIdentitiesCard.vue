<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Bot, Pencil, Plus, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { WorkloadIdentityResponse } from '@/api/generated'
import {
  deleteWorkloadIdentityMutation,
  listAccountsOptions,
  listWorkloadIdentitiesOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { notifyFailure } from '@/lib/configuration'
import { useSession } from '@/lib/session'
import WorkloadIdentityFormSheet from './WorkloadIdentityFormSheet.vue'

const { t } = useI18n()
const { can } = useSession()
const canManage = computed(() => can('identity.manage'))
const trusts = useQuery(listWorkloadIdentitiesOptions())
const accounts = useQuery(listAccountsOptions())
const remove = useMutation(deleteWorkloadIdentityMutation())
const names = computed(
  () => new Map((accounts.data.value ?? []).map((account) => [account.id, account.username])),
)

const editing = ref<WorkloadIdentityResponse>()
const formOpen = ref(false)
function openForm(trust?: WorkloadIdentityResponse) {
  editing.value = trust
  formOpen.value = true
}

const removing = ref<WorkloadIdentityResponse | null>(null)
const removeOpen = computed({
  get: () => removing.value !== null,
  set: (open) => {
    if (!open) {
      removing.value = null
    }
  },
})
function confirmRemove() {
  const trust = removing.value
  if (!trust) {
    return
  }
  remove.mutate(
    { path: { id: trust.id } },
    {
      onSuccess: () => {
        toast.success(t('workloads.deleted'))
        removing.value = null
        void trusts.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <Card>
    <CardHeader class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
      <div class="flex flex-col gap-1.5">
        <CardTitle class="flex items-center gap-2">
          <Bot class="size-4" aria-hidden="true" />
          {{ t('workloads.title') }}
        </CardTitle>
        <CardDescription>{{ t('workloads.description') }}</CardDescription>
      </div>
      <Button v-if="canManage" size="sm" variant="outline" @click="openForm()">
        <Plus data-icon="inline-start" aria-hidden="true" />
        {{ t('workloads.new') }}
      </Button>
    </CardHeader>
    <CardContent>
      <p v-if="!trusts.data.value?.length" class="text-muted-foreground text-sm">
        {{ t('workloads.empty') }}
      </p>
      <ul v-else class="divide-y rounded-md border">
        <li
          v-for="trust in trusts.data.value"
          :key="trust.id"
          class="flex items-start justify-between gap-3 p-3"
        >
          <div class="flex min-w-0 flex-col gap-1">
            <span class="flex flex-wrap items-center gap-2 text-sm font-medium">
              <code class="font-mono">{{ trust.id }}</code>
              <span class="text-muted-foreground">→</span>
              {{ names.get(trust.account_id) ?? trust.account_id }}
              <Badge v-if="!trust.enabled" variant="outline">{{ t('workloads.disabled') }}</Badge>
            </span>
            <span class="text-muted-foreground text-xs break-all">{{ trust.issuer }}</span>
            <span class="flex flex-wrap gap-1">
              <Badge variant="secondary" class="font-mono">{{ trust.subject }}</Badge>
              <Badge
                v-for="(value, name) in trust.claims"
                :key="name"
                variant="outline"
                class="font-mono"
                >{{ name }}={{ value }}</Badge
              >
              <Badge variant="outline">{{
                t('workloads.minutes', { count: trust.session_minutes })
              }}</Badge>
            </span>
          </div>
          <span v-if="canManage" class="flex shrink-0 flex-col gap-1 sm:flex-row">
            <Button
              variant="ghost"
              size="icon-sm"
              :aria-label="t('common.edit')"
              :title="t('common.edit')"
              @click="openForm(trust)"
            >
              <Pencil aria-hidden="true" />
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              :aria-label="t('common.delete')"
              :title="t('common.delete')"
              @click="removing = trust"
            >
              <Trash2 aria-hidden="true" />
            </Button>
          </span>
        </li>
      </ul>
    </CardContent>
    <WorkloadIdentityFormSheet v-model:open="formOpen" :trust="editing" @saved="trusts.refetch()" />
    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="t('workloads.deleteTitle', { id: removing?.id ?? '' })"
      :description="t('workloads.deleteDetail')"
      :confirm-label="t('common.delete')"
      destructive
      :busy="remove.isPending.value"
      @confirm="confirmRemove"
    />
  </Card>
</template>
