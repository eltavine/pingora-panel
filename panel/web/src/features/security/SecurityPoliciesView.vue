<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Pencil, Plus, ShieldBan, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { SecurityPolicyView } from '@/api/generated'
import {
  deleteSecurityPolicyMutation,
  listSecurityPoliciesOptions,
  listSitesOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import PageHeader from '@/components/PageHeader.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { changeHeaders, notifyFailure, useRefreshConfiguration } from '@/lib/configuration'
import { restrictions } from './forms'
import { restrictionIcons } from './presentation'
import SecurityPolicyFormSheet from './SecurityPolicyFormSheet.vue'

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const policies = useQuery(listSecurityPoliciesOptions())
const sites = useQuery(listSitesOptions({ query: { limit: 500 } }))
const remove = useMutation(deleteSecurityPolicyMutation())

const siteNames = computed(
  () => new Map((sites.data.value?.items ?? []).map((site) => [site.id, site.name])),
)
const ids = computed(() => (policies.data.value ?? []).map((policy) => policy.id))

const editing = ref<SecurityPolicyView | undefined>()
const formOpen = ref(false)
function openForm(policy?: SecurityPolicyView) {
  editing.value = policy
  formOpen.value = true
}

const removing = ref<SecurityPolicyView | null>(null)
const removeOpen = computed({
  get: () => removing.value !== null,
  set: (open) => {
    if (!open) {
      removing.value = null
    }
  },
})
function confirmRemove() {
  const target = removing.value
  if (!target) {
    return
  }
  remove.mutate(
    { path: { id: target.id }, headers: changeHeaders(target.etag) },
    {
      onSuccess: () => {
        toast.success(t('security.deleted'))
        removing.value = null
        void refresh()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="ShieldBan"
      :title="t('security.title')"
      :description="t('security.description')"
    />

    <Card>
      <CardHeader>
        <CardTitle class="flex items-center gap-2">
          <ShieldBan class="size-4" aria-hidden="true" />
          {{ t('security.listTitle') }}
        </CardTitle>
        <CardDescription>{{ t('security.listHint') }}</CardDescription>
        <CardAction>
          <Button size="sm" @click="openForm()">
            <Plus data-icon="inline-start" aria-hidden="true" />
            {{ t('security.new') }}
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent>
        <ApiFailureAlert
          v-if="policies.isError.value && !policies.data.value"
          :error="policies.error.value"
          retryable
          @retry="policies.refetch()"
        />
        <Skeleton v-else-if="policies.isPending.value" class="h-24 w-full" />
        <Empty v-else-if="(policies.data.value ?? []).length === 0" class="border">
          <EmptyHeader>
            <EmptyMedia variant="icon"><ShieldBan aria-hidden="true" /></EmptyMedia>
            <EmptyTitle>{{ t('security.emptyTitle') }}</EmptyTitle>
            <EmptyDescription>{{ t('security.emptyDetail') }}</EmptyDescription>
          </EmptyHeader>
        </Empty>
        <div v-else class="rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{{ t('security.id') }}</TableHead>
                <TableHead>{{ t('security.restricts') }}</TableHead>
                <TableHead>{{ t('security.usedBy') }}</TableHead>
                <TableHead class="w-20"
                  ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
                >
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="policy in policies.data.value" :key="policy.id">
                <TableCell class="font-mono text-xs">{{ policy.id }}</TableCell>
                <TableCell>
                  <div class="flex flex-wrap gap-1">
                    <Badge
                      v-for="name in restrictions(policy)"
                      :key="name"
                      variant="outline"
                      class="gap-1"
                    >
                      <component :is="restrictionIcons[name]" aria-hidden="true" />
                      {{ t(`security.restrictions.${name}`) }}
                    </Badge>
                    <span
                      v-if="restrictions(policy).length === 0"
                      class="text-muted-foreground text-xs"
                      >{{ t('security.nothing') }}</span
                    >
                  </div>
                </TableCell>
                <TableCell>
                  <div v-if="policy.used_by.length > 0" class="flex flex-wrap gap-x-2 gap-y-1">
                    <RouterLink
                      v-for="site in policy.used_by"
                      :key="site"
                      :to="`/sites/${site}`"
                      class="text-sm hover:underline"
                    >
                      {{ siteNames.get(site) ?? site }}
                    </RouterLink>
                  </div>
                  <span v-else class="text-muted-foreground text-xs">{{
                    t('security.unused')
                  }}</span>
                </TableCell>
                <TableCell>
                  <div class="flex flex-col items-end gap-1 sm:flex-row sm:justify-end">
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      :aria-label="t('common.edit')"
                      @click="openForm(policy)"
                    >
                      <Pencil aria-hidden="true" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      :aria-label="t('common.delete')"
                      :title="policy.used_by.length > 0 ? t('security.inUse') : undefined"
                      :disabled="policy.used_by.length > 0"
                      @click="removing = policy"
                    >
                      <Trash2 aria-hidden="true" />
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
      </CardContent>
    </Card>

    <SecurityPolicyFormSheet v-model:open="formOpen" :policy="editing" :taken="ids" />

    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="t('security.confirmDeleteTitle', { id: removing?.id ?? '' })"
      :description="t('security.confirmDeleteDetail')"
      :confirm-label="t('common.delete')"
      destructive
      :busy="remove.isPending.value"
      @confirm="confirmRemove"
    />
  </div>
</template>
