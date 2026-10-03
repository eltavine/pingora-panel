<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Trash2, UserRoundKey } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { AcmeAccountView } from '@/api/generated'
import {
  deleteAcmeAccountMutation,
  listAcmeAccountsOptions,
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
import { directoryHost, knownDirectory } from './presentation'

defineProps<{ canManage: boolean }>()
const emit = defineEmits<{ register: [] }>()

const { t, d } = useI18n()
const accounts = useQuery(listAcmeAccountsOptions())
const remove = useMutation(deleteAcmeAccountMutation())

function caName(account: AcmeAccountView): string {
  const known = knownDirectory(account.directory)
  return known ? t(`certificates.acme.directories.${known.id}`) : directoryHost(account.directory)
}

const removing = ref<AcmeAccountView | null>(null)
const removeOpen = computed({
  get: () => removing.value !== null,
  set: (open) => {
    if (!open) {
      removing.value = null
    }
  },
})
function confirmRemove() {
  const account = removing.value
  if (!account) {
    return
  }
  remove.mutate(
    { path: { id: account.id }, headers: changeHeaders(account.etag) },
    {
      onSuccess: () => {
        toast.success(t('certificates.acme.accountDeleted', { id: account.id }))
        removing.value = null
        void accounts.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <ApiFailureAlert
    v-if="accounts.isError.value && !accounts.data.value"
    :error="accounts.error.value"
    retryable
    @retry="accounts.refetch()"
  />
  <div v-else-if="accounts.isPending.value" class="flex flex-col gap-2">
    <Skeleton v-for="index in 2" :key="index" class="h-12 w-full" />
  </div>
  <Empty v-else-if="(accounts.data.value ?? []).length === 0" class="border">
    <EmptyHeader>
      <EmptyMedia variant="icon"><UserRoundKey aria-hidden="true" /></EmptyMedia>
      <EmptyTitle>{{ t('certificates.acme.noAccounts') }}</EmptyTitle>
      <EmptyDescription>{{ t('certificates.acme.noAccountsDetail') }}</EmptyDescription>
    </EmptyHeader>
    <EmptyContent v-if="canManage" class="flex-row justify-center">
      <Button size="sm" @click="emit('register')">
        <UserRoundKey data-icon="inline-start" aria-hidden="true" />
        {{ t('certificates.acme.register') }}
      </Button>
    </EmptyContent>
  </Empty>
  <div v-else class="rounded-lg border">
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>{{ t('certificates.acme.accountId') }}</TableHead>
          <TableHead>{{ t('certificates.acme.ca') }}</TableHead>
          <TableHead class="hidden md:table-cell">{{ t('certificates.acme.emails') }}</TableHead>
          <TableHead class="hidden lg:table-cell">{{
            t('certificates.acme.registeredAt')
          }}</TableHead>
          <TableHead class="w-16"
            ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
          >
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="account in accounts.data.value" :key="account.id">
          <TableCell class="align-top">
            <span class="font-mono text-sm font-medium">{{ account.id }}</span>
            <Badge
              v-if="account.external_account_key_id"
              variant="secondary"
              class="ml-2"
              :title="account.external_account_key_id"
              >EAB</Badge
            >
          </TableCell>
          <TableCell class="max-w-64 align-top">
            <span class="block text-sm">{{ caName(account) }}</span>
            <span class="text-muted-foreground block truncate font-mono text-xs">{{
              account.directory
            }}</span>
          </TableCell>
          <TableCell class="hidden align-top text-sm md:table-cell">{{
            account.contact.length > 0 ? account.contact.join(', ') : '–'
          }}</TableCell>
          <TableCell class="hidden align-top text-sm lg:table-cell">{{
            d(new Date(account.created_at), 'datetime')
          }}</TableCell>
          <TableCell class="align-top">
            <span v-if="canManage" class="flex justify-end">
              <Button
                variant="ghost"
                size="icon-sm"
                :aria-label="t('common.delete')"
                :title="t('common.delete')"
                @click="removing = account"
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
    :title="t('certificates.acme.deleteAccountTitle', { id: removing?.id ?? '' })"
    :description="t('certificates.acme.deleteAccountDetail')"
    :confirm-label="t('common.delete')"
    destructive
    :busy="remove.isPending.value"
    @confirm="confirmRemove"
  />
</template>
