<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import { CalendarSync, CircleStop, RotateCw, UserRoundKey } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { AutomaticCertificateView } from '@/api/generated'
import {
  deleteAutomaticCertificateMutation,
  listAcmeAccountsOptions,
  listAutomaticCertificatesOptions,
  listCertificatesQueryKey,
  renewAutomaticCertificateMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
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
import { changeHeaders, notifyFailure, plainHeaders } from '@/lib/configuration'
import { ISSUANCE_TONES } from './presentation'

/** How often pending certificates are looked at again. */
const PENDING_REFRESH_MS = 5000
const SHOWN_NAMES = 3

defineProps<{ canManage: boolean }>()
const emit = defineEmits<{ request: []; register: [] }>()

const { t, d } = useI18n()
const queryClient = useQueryClient()
const automatic = useQuery({
  ...listAutomaticCertificatesOptions(),
  refetchInterval: (query) =>
    (query.state.data ?? []).some((certificate) => certificate.state === 'pending')
      ? PENDING_REFRESH_MS
      : false,
})
const accounts = useQuery(listAcmeAccountsOptions())
const hasAccounts = computed(() => (accounts.data.value ?? []).length > 0)
const renew = useMutation(renewAutomaticCertificateMutation())
const remove = useMutation(deleteAutomaticCertificateMutation())

watch(automatic.data, (current, previous) => {
  const issued = (current ?? []).some(
    (certificate) =>
      certificate.state === 'issued' &&
      previous?.find((before) => before.id === certificate.id)?.state !== 'issued',
  )
  if (issued && previous) {
    void queryClient.invalidateQueries({ queryKey: listCertificatesQueryKey() })
  }
})

function when(certificate: AutomaticCertificateView): string {
  const at = d(new Date(certificate.renew_after), 'datetime')
  switch (certificate.state) {
    case 'pending':
      return t('certificates.acme.issuing')
    case 'failing':
      return t('certificates.acme.nextAttempt', { at })
    default:
      return t('certificates.acme.renewsAt', { at })
  }
}

function renewNow(certificate: AutomaticCertificateView) {
  renew.mutate(
    { path: { id: certificate.id }, headers: plainHeaders() },
    {
      onSuccess: () => {
        toast.success(t('certificates.acme.renewing', { id: certificate.id }))
        void automatic.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

const stopping = ref<AutomaticCertificateView | null>(null)
const stopOpen = computed({
  get: () => stopping.value !== null,
  set: (open) => {
    if (!open) {
      stopping.value = null
    }
  },
})
function confirmStop() {
  const certificate = stopping.value
  if (!certificate) {
    return
  }
  remove.mutate(
    { path: { id: certificate.id }, headers: changeHeaders(certificate.etag) },
    {
      onSuccess: () => {
        toast.success(t('certificates.acme.stopped', { id: certificate.id }))
        stopping.value = null
        void automatic.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <ApiFailureAlert
    v-if="automatic.isError.value && !automatic.data.value"
    :error="automatic.error.value"
    retryable
    @retry="automatic.refetch()"
  />
  <div v-else-if="automatic.isPending.value" class="flex flex-col gap-2">
    <Skeleton v-for="index in 3" :key="index" class="h-12 w-full" />
  </div>
  <Empty v-else-if="(automatic.data.value ?? []).length === 0" class="border">
    <EmptyHeader>
      <EmptyMedia variant="icon"><CalendarSync aria-hidden="true" /></EmptyMedia>
      <EmptyTitle>{{ t('certificates.acme.emptyTitle') }}</EmptyTitle>
      <EmptyDescription>{{
        hasAccounts ? t('certificates.acme.emptyDetail') : t('certificates.acme.noAccountsDetail')
      }}</EmptyDescription>
    </EmptyHeader>
    <EmptyContent v-if="canManage" class="flex-row justify-center">
      <Button v-if="hasAccounts" size="sm" @click="emit('request')">
        <CalendarSync data-icon="inline-start" aria-hidden="true" />
        {{ t('certificates.acme.request') }}
      </Button>
      <Button v-else size="sm" @click="emit('register')">
        <UserRoundKey data-icon="inline-start" aria-hidden="true" />
        {{ t('certificates.acme.register') }}
      </Button>
    </EmptyContent>
  </Empty>
  <div v-else class="rounded-lg border">
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>{{ t('certificates.columns.certificate') }}</TableHead>
          <TableHead>{{ t('certificates.columns.status') }}</TableHead>
          <TableHead class="hidden md:table-cell">{{ t('certificates.acme.account') }}</TableHead>
          <TableHead class="w-10 sm:w-24"
            ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
          >
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="certificate in automatic.data.value" :key="certificate.id">
          <TableCell class="max-w-72 align-top">
            <span class="font-mono text-sm font-medium">{{ certificate.id }}</span>
            <span class="mt-1 flex flex-wrap gap-1">
              <Badge
                v-for="name in certificate.names.slice(0, SHOWN_NAMES)"
                :key="name"
                variant="outline"
                class="font-mono"
                >{{ name }}</Badge
              >
              <Badge v-if="certificate.names.length > SHOWN_NAMES" variant="outline"
                >+{{ certificate.names.length - SHOWN_NAMES }}</Badge
              >
            </span>
          </TableCell>
          <TableCell class="max-w-80 align-top">
            <StatusIndicator
              :tone="ISSUANCE_TONES[certificate.state]"
              :label="t(`certificates.acme.state.${certificate.state}`)"
            />
            <span class="text-muted-foreground block text-xs">{{ when(certificate) }}</span>
            <span
              v-if="certificate.last_error"
              class="text-destructive mt-1 line-clamp-2 block text-xs break-words"
              :title="certificate.last_error.message"
              >{{
                t(
                  'certificates.acme.failures',
                  { count: certificate.failures, message: certificate.last_error.message },
                  certificate.failures,
                )
              }}</span
            >
          </TableCell>
          <TableCell class="hidden align-top font-mono text-xs md:table-cell">{{
            certificate.account
          }}</TableCell>
          <TableCell class="align-top">
            <span v-if="canManage" class="flex flex-col items-end gap-1 sm:flex-row sm:justify-end">
              <Button
                variant="ghost"
                size="icon-sm"
                :aria-label="t('certificates.acme.renewNow')"
                :title="t('certificates.acme.renewNow')"
                :disabled="renew.isPending.value"
                @click="renewNow(certificate)"
              >
                <RotateCw aria-hidden="true" />
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                :aria-label="t('certificates.acme.stop')"
                :title="t('certificates.acme.stop')"
                @click="stopping = certificate"
              >
                <CircleStop aria-hidden="true" />
              </Button>
            </span>
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  </div>
  <ConfirmDialog
    v-model:open="stopOpen"
    :icon="CircleStop"
    :title="t('certificates.acme.stopTitle', { id: stopping?.id ?? '' })"
    :description="t('certificates.acme.stopDetail')"
    :confirm-label="t('certificates.acme.stop')"
    destructive
    :busy="remove.isPending.value"
    @confirm="confirmStop"
  />
</template>
