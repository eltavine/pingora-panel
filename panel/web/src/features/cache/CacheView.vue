<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { DatabaseZap, Filter, Pencil, Plus, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { CachePolicyView } from '@/api/generated'
import {
  deleteCachePolicyMutation,
  listCachePoliciesOptions,
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
import CachePolicyFormSheet from './CachePolicyFormSheet.vue'
import CacheStatsCard from './CacheStatsCard.vue'
import CacheStoreCard from './CacheStoreCard.vue'
import { lifetime, statusTimes } from './forms'

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const policies = useQuery(listCachePoliciesOptions())
const sites = useQuery(listSitesOptions({ query: { limit: 500 } }))
const remove = useMutation(deleteCachePolicyMutation())

const siteNames = computed(
  () => new Map((sites.data.value?.items ?? []).map((site) => [site.id, site.name])),
)
const ids = computed(() => (policies.data.value ?? []).map((policy) => policy.id))

const editing = ref<CachePolicyView | undefined>()
const formOpen = ref(false)
function openForm(policy?: CachePolicyView) {
  editing.value = policy
  formOpen.value = true
}

const removing = ref<CachePolicyView | null>(null)
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
        toast.success(t('cache.deleted'))
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
      :icon="DatabaseZap"
      :title="t('cache.title')"
      :description="t('cache.description')"
    />

    <CacheStatsCard />
    <CacheStoreCard />

    <Card>
      <CardHeader>
        <CardTitle class="flex items-center gap-2">
          <DatabaseZap class="size-4" aria-hidden="true" />
          {{ t('cache.listTitle') }}
        </CardTitle>
        <CardDescription>{{ t('cache.listHint') }}</CardDescription>
        <CardAction>
          <Button size="sm" @click="openForm()">
            <Plus data-icon="inline-start" aria-hidden="true" />
            {{ t('cache.new') }}
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
            <EmptyMedia variant="icon"><DatabaseZap aria-hidden="true" /></EmptyMedia>
            <EmptyTitle>{{ t('cache.emptyTitle') }}</EmptyTitle>
            <EmptyDescription>{{ t('cache.emptyDetail') }}</EmptyDescription>
          </EmptyHeader>
        </Empty>
        <div v-else class="rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{{ t('cache.id') }}</TableHead>
                <TableHead>{{ t('cache.lifetime') }}</TableHead>
                <TableHead>{{ t('cache.usedBy') }}</TableHead>
                <TableHead class="w-20"
                  ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
                >
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="policy in policies.data.value" :key="policy.id">
                <TableCell class="font-mono text-xs">
                  <span class="flex flex-wrap items-center gap-2">
                    {{ policy.id }}
                    <Badge v-if="policy.enabled === false" variant="outline">{{
                      t('cache.disabled')
                    }}</Badge>
                  </span>
                </TableCell>
                <TableCell>
                  <div class="flex flex-wrap items-center gap-1">
                    <Badge variant="outline" class="font-mono">{{
                      lifetime(policy) || t('cache.originsLifetime')
                    }}</Badge>
                    <span
                      v-if="statusTimes(policy)"
                      class="text-muted-foreground font-mono text-xs"
                      >{{ statusTimes(policy) }}</span
                    >
                    <Badge
                      v-if="(policy.bypass ?? []).length > 0"
                      variant="outline"
                      class="gap-1"
                      :title="t('cache.sections.bypass')"
                    >
                      <Filter aria-hidden="true" />
                      {{ t('cache.bypassCount', (policy.bypass ?? []).length) }}
                    </Badge>
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
                  <span v-else class="text-muted-foreground text-xs">{{ t('cache.unused') }}</span>
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
                      :title="policy.used_by.length > 0 ? t('cache.inUse') : undefined"
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

    <CachePolicyFormSheet v-model:open="formOpen" :policy="editing" :taken="ids" />

    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="t('cache.confirmDeleteTitle', { id: removing?.id ?? '' })"
      :description="t('cache.confirmDeleteDetail')"
      :confirm-label="t('common.delete')"
      destructive
      :busy="remove.isPending.value"
      @confirm="confirmRemove"
    />
  </div>
</template>
