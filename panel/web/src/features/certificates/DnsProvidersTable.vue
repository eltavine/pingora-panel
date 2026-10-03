<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Network, Pencil, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { DnsProviderView } from '@/api/generated'
import {
  deleteDnsProviderMutation,
  listDnsProvidersOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from '@/components/ui/empty'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { changeHeaders, notifyFailure } from '@/lib/configuration'

defineProps<{ canManage: boolean }>()
const emit = defineEmits<{ add: []; edit: [provider: DnsProviderView] }>()

const { t } = useI18n()
const providers = useQuery(listDnsProvidersOptions())
const remove = useMutation(deleteDnsProviderMutation())

const removing = ref<DnsProviderView | null>(null)
const removeOpen = computed({
  get: () => removing.value !== null,
  set: (open) => {
    if (!open) {
      removing.value = null
    }
  },
})
function confirmRemove() {
  const provider = removing.value
  if (!provider) {
    return
  }
  remove.mutate(
    { path: { id: provider.id }, headers: changeHeaders(provider.etag) },
    {
      onSuccess: () => {
        toast.success(t('certificates.dns.deleted', { id: provider.id }))
        removing.value = null
        void providers.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <ApiFailureAlert
    v-if="providers.isError.value && !providers.data.value"
    :error="providers.error.value"
    retryable
    @retry="providers.refetch()"
  />
  <div v-else-if="providers.isPending.value" class="flex flex-col gap-2">
    <Skeleton v-for="index in 2" :key="index" class="h-12 w-full" />
  </div>
  <Empty v-else-if="(providers.data.value ?? []).length === 0" class="border">
    <EmptyHeader>
      <EmptyMedia variant="icon"><Network aria-hidden="true" /></EmptyMedia>
      <EmptyTitle>{{ t('certificates.dns.emptyTitle') }}</EmptyTitle>
      <EmptyDescription>{{ t('certificates.dns.emptyDetail') }}</EmptyDescription>
    </EmptyHeader>
    <EmptyContent v-if="canManage" class="flex-row justify-center">
      <Button size="sm" @click="emit('add')">
        <Network data-icon="inline-start" aria-hidden="true" />
        {{ t('certificates.dns.add') }}
      </Button>
    </EmptyContent>
  </Empty>
  <div v-else class="rounded-lg border">
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>{{ t('certificates.dns.id') }}</TableHead>
          <TableHead>{{ t('certificates.dns.server') }}</TableHead>
          <TableHead class="hidden md:table-cell">{{ t('certificates.dns.zones') }}</TableHead>
          <TableHead class="hidden lg:table-cell">{{ t('certificates.dns.keyName') }}</TableHead>
          <TableHead class="w-10 sm:w-24"
            ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
          >
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="provider in providers.data.value" :key="provider.id">
          <TableCell class="align-top">
            <span class="font-mono text-sm font-medium">{{ provider.id }}</span>
            <Badge variant="secondary" class="ml-2">RFC 2136</Badge>
          </TableCell>
          <TableCell class="align-top font-mono text-xs">{{ provider.rfc2136.server }}</TableCell>
          <TableCell class="hidden align-top md:table-cell">
            <span class="flex flex-wrap gap-1">
              <Badge
                v-for="zone in provider.rfc2136.zones"
                :key="zone"
                variant="outline"
                class="font-mono"
                >{{ zone }}</Badge
              >
            </span>
          </TableCell>
          <TableCell class="hidden align-top font-mono text-xs lg:table-cell"
            >{{ provider.rfc2136.key_name }} · {{ provider.rfc2136.algorithm }}</TableCell
          >
          <TableCell class="align-top">
            <span v-if="canManage" class="flex flex-col items-end gap-1 sm:flex-row sm:justify-end">
              <Button
                variant="ghost"
                size="icon-sm"
                :aria-label="t('common.edit')"
                :title="t('common.edit')"
                @click="emit('edit', provider)"
              >
                <Pencil aria-hidden="true" />
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                :aria-label="t('common.delete')"
                :title="t('common.delete')"
                @click="removing = provider"
              >
                <Trash2 aria-hidden="true" />
              </Button>
            </span>
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  </div>
  <ConfirmDialog
    v-model:open="removeOpen"
    :icon="Trash2"
    :title="t('certificates.dns.deleteTitle', { id: removing?.id ?? '' })"
    :description="t('certificates.dns.deleteDetail')"
    :confirm-label="t('common.delete')"
    destructive
    :busy="remove.isPending.value"
    @confirm="confirmRemove"
  />
</template>
