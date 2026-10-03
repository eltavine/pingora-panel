<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import {
  CalendarSync,
  Eye,
  FileBadge,
  RefreshCw,
  Sparkles,
  Trash2,
  Upload,
  UserRoundKey,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { CertificateView } from '@/api/generated'
import {
  deleteCertificateMutation,
  listAcmeAccountsOptions,
  listAcmeAccountsQueryKey,
  listAutomaticCertificatesOptions,
  listAutomaticCertificatesQueryKey,
  listCertificatesOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import PageHeader from '@/components/PageHeader.vue'
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { changeHeaders, notifyFailure } from '@/lib/configuration'
import { useSession } from '@/lib/session'
import AcmeAccountSheet from './AcmeAccountSheet.vue'
import AcmeAccountsTable from './AcmeAccountsTable.vue'
import AutomaticCertificateSheet from './AutomaticCertificateSheet.vue'
import AutomaticCertificatesTable from './AutomaticCertificatesTable.vue'
import CertificateDetailsSheet from './CertificateDetailsSheet.vue'
import CertificateGenerateSheet from './CertificateGenerateSheet.vue'
import CertificateUploadSheet from './CertificateUploadSheet.vue'
import { STATUS_TONES, daysLeft } from './presentation'

const SHOWN_NAMES = 3
const TABS = ['inventory', 'automatic', 'accounts'] as const
type Tab = (typeof TABS)[number]

const { t, d } = useI18n()
const { can } = useSession()
const route = useRoute()
const router = useRouter()
const queryClient = useQueryClient()
const canManage = computed(() => can('certificate.manage'))
const certificates = useQuery(listCertificatesOptions())
const remove = useMutation(deleteCertificateMutation())
const ids = computed(() => (certificates.data.value ?? []).map((certificate) => certificate.id))

const tab = ref<Tab>(TABS.find((candidate) => candidate === route.query.tab) ?? 'inventory')
watch(tab, (current) => {
  void router.replace({
    query: { ...route.query, tab: current === 'inventory' ? undefined : current },
  })
})
const accounts = useQuery(listAcmeAccountsOptions())
const automatic = useQuery(listAutomaticCertificatesOptions())
const accountIds = computed(() => (accounts.data.value ?? []).map((account) => account.id))
const automaticIds = computed(() =>
  (automatic.data.value ?? []).map((certificate) => certificate.id),
)
const requesting = ref(false)
const registering = ref(false)
function register() {
  tab.value = 'accounts'
  registering.value = true
}
function refresh(queryKey: readonly unknown[]) {
  void queryClient.invalidateQueries({ queryKey })
}

const uploading = ref(false)
const generating = ref(false)
const replacing = ref<CertificateView>()
const replaceOpen = ref(false)
const shown = ref<CertificateView>()
const detailsOpen = ref(false)

function openUpload(certificate?: CertificateView) {
  if (certificate) {
    replacing.value = certificate
    replaceOpen.value = true
  } else {
    uploading.value = true
  }
}

function openDetails(certificate: CertificateView) {
  shown.value = certificate
  detailsOpen.value = true
}

function remaining(certificate: CertificateView): string {
  const days = daysLeft(certificate.not_after)
  if (certificate.status === 'expired') {
    return t('certificates.expiredAgo', { count: -days }, -days)
  }
  if (certificate.status === 'not_yet_valid') {
    return d(new Date(certificate.not_before), 'datetime')
  }
  return t('certificates.daysLeft', { count: days }, days)
}

const removing = ref<CertificateView | null>(null)
const removeOpen = computed({
  get: () => removing.value !== null,
  set: (open) => {
    if (!open) {
      removing.value = null
    }
  },
})
function confirmRemove() {
  const certificate = removing.value
  if (!certificate) {
    return
  }
  remove.mutate(
    { path: { id: certificate.id }, headers: changeHeaders(certificate.etag) },
    {
      onSuccess: () => {
        toast.success(t('certificates.deleted'))
        removing.value = null
        void certificates.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="FileBadge"
      :title="t('certificates.title')"
      :description="t('certificates.description')"
    >
      <template v-if="canManage" #actions>
        <template v-if="tab === 'inventory'">
          <Button size="sm" variant="outline" @click="generating = true">
            <Sparkles data-icon="inline-start" aria-hidden="true" />
            {{ t('certificates.generate') }}
          </Button>
          <Button size="sm" @click="openUpload()">
            <Upload data-icon="inline-start" aria-hidden="true" />
            {{ t('certificates.upload') }}
          </Button>
        </template>
        <Button
          v-else-if="tab === 'automatic'"
          size="sm"
          :disabled="accountIds.length === 0"
          @click="requesting = true"
        >
          <CalendarSync data-icon="inline-start" aria-hidden="true" />
          {{ t('certificates.acme.request') }}
        </Button>
        <Button v-else size="sm" @click="registering = true">
          <UserRoundKey data-icon="inline-start" aria-hidden="true" />
          {{ t('certificates.acme.register') }}
        </Button>
      </template>
    </PageHeader>

    <Tabs v-model="tab" class="gap-4">
      <TabsList :aria-label="t('certificates.title')">
        <TabsTrigger value="inventory">
          <FileBadge aria-hidden="true" />
          {{ t('certificates.tabs.inventory') }}
        </TabsTrigger>
        <TabsTrigger value="automatic">
          <CalendarSync aria-hidden="true" />
          {{ t('certificates.tabs.automatic') }}
        </TabsTrigger>
        <TabsTrigger value="accounts">
          <UserRoundKey aria-hidden="true" />
          {{ t('certificates.tabs.accounts') }}
        </TabsTrigger>
      </TabsList>
      <TabsContent value="inventory">
        <ApiFailureAlert
          v-if="certificates.isError.value && !certificates.data.value"
          :error="certificates.error.value"
          retryable
          @retry="certificates.refetch()"
        />
        <div v-else-if="certificates.isPending.value" class="flex flex-col gap-2">
          <Skeleton v-for="index in 3" :key="index" class="h-12 w-full" />
        </div>
        <Empty v-else-if="ids.length === 0" class="border">
          <EmptyHeader>
            <EmptyMedia variant="icon"><FileBadge aria-hidden="true" /></EmptyMedia>
            <EmptyTitle>{{ t('certificates.emptyTitle') }}</EmptyTitle>
            <EmptyDescription>{{ t('certificates.emptyDetail') }}</EmptyDescription>
          </EmptyHeader>
          <EmptyContent v-if="canManage" class="flex-row justify-center">
            <Button size="sm" variant="outline" @click="generating = true">
              <Sparkles data-icon="inline-start" aria-hidden="true" />
              {{ t('certificates.generate') }}
            </Button>
            <Button size="sm" @click="openUpload()">
              <Upload data-icon="inline-start" aria-hidden="true" />
              {{ t('certificates.upload') }}
            </Button>
          </EmptyContent>
        </Empty>
        <div v-else class="rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{{ t('certificates.columns.certificate') }}</TableHead>
                <TableHead class="hidden sm:table-cell">{{
                  t('certificates.columns.names')
                }}</TableHead>
                <TableHead>{{ t('certificates.columns.status') }}</TableHead>
                <TableHead class="hidden md:table-cell">{{
                  t('certificates.columns.expires')
                }}</TableHead>
                <TableHead class="hidden lg:table-cell">{{
                  t('certificates.columns.issuer')
                }}</TableHead>
                <TableHead class="w-28"
                  ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
                >
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="certificate in certificates.data.value" :key="certificate.id">
                <TableCell class="max-w-64 align-top">
                  <span class="flex flex-wrap items-center gap-2 font-medium">
                    <span class="font-mono text-sm">{{ certificate.id }}</span>
                    <Badge variant="secondary">{{
                      t(`certificates.source.${certificate.source}`)
                    }}</Badge>
                  </span>
                  <span class="text-muted-foreground block truncate text-xs">{{
                    certificate.subject
                  }}</span>
                </TableCell>
                <TableCell class="hidden align-top sm:table-cell">
                  <span class="flex flex-wrap gap-1">
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
                <TableCell class="align-top">
                  <StatusIndicator
                    :tone="STATUS_TONES[certificate.status]"
                    :label="t(`certificates.status.${certificate.status}`)"
                  />
                  <span class="text-muted-foreground block text-xs">{{
                    remaining(certificate)
                  }}</span>
                </TableCell>
                <TableCell class="hidden align-top text-sm md:table-cell">
                  {{ d(new Date(certificate.not_after), 'datetime') }}
                </TableCell>
                <TableCell
                  class="text-muted-foreground hidden max-w-56 align-top text-xs lg:table-cell"
                >
                  <span class="block truncate">{{ certificate.issuer }}</span>
                </TableCell>
                <TableCell class="align-top">
                  <span class="flex justify-end gap-1">
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      :aria-label="t('certificates.details')"
                      :title="t('certificates.details')"
                      @click="openDetails(certificate)"
                    >
                      <Eye aria-hidden="true" />
                    </Button>
                    <template v-if="canManage">
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        :aria-label="t('certificates.replace')"
                        :title="t('certificates.replace')"
                        @click="openUpload(certificate)"
                      >
                        <RefreshCw aria-hidden="true" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        :aria-label="t('common.delete')"
                        :title="t('common.delete')"
                        @click="removing = certificate"
                      >
                        <Trash2 aria-hidden="true" />
                      </Button>
                    </template>
                  </span>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
      </TabsContent>
      <TabsContent value="automatic">
        <AutomaticCertificatesTable
          :can-manage="canManage"
          @request="requesting = true"
          @register="register"
        />
      </TabsContent>
      <TabsContent value="accounts">
        <AcmeAccountsTable :can-manage="canManage" @register="registering = true" />
      </TabsContent>
    </Tabs>

    <AutomaticCertificateSheet
      v-model:open="requesting"
      :accounts="accounts.data.value ?? []"
      :taken="automaticIds"
      @saved="refresh(listAutomaticCertificatesQueryKey())"
    />
    <AcmeAccountSheet
      v-model:open="registering"
      :taken="accountIds"
      @saved="refresh(listAcmeAccountsQueryKey())"
    />
    <CertificateUploadSheet v-model:open="uploading" :taken="ids" @saved="certificates.refetch()" />
    <CertificateUploadSheet
      v-model:open="replaceOpen"
      :certificate="replacing"
      :taken="ids"
      @saved="certificates.refetch()"
    />
    <CertificateGenerateSheet
      v-model:open="generating"
      :taken="ids"
      @saved="certificates.refetch()"
    />
    <CertificateDetailsSheet v-model:open="detailsOpen" :certificate="shown" />
    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="t('certificates.deleteTitle', { id: removing?.id ?? '' })"
      :description="t('certificates.deleteDetail')"
      :confirm-label="t('common.delete')"
      destructive
      :busy="remove.isPending.value"
      @confirm="confirmRemove"
    />
  </div>
</template>
