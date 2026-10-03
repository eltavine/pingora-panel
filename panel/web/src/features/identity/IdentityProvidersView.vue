<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Fingerprint, KeyRound, Pencil, Plus, Trash2, UserPlus } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { IdentityProviderResponse } from '@/api/generated'
import {
  deleteIdentityProviderMutation,
  listIdentityProvidersOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import PageHeader from '@/components/PageHeader.vue'
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
import { notifyFailure } from '@/lib/configuration'
import { useSession } from '@/lib/session'
import IdentityProviderFormSheet from './IdentityProviderFormSheet.vue'
import PasswordSignInCard from './PasswordSignInCard.vue'

const { t } = useI18n()
const { can } = useSession()
const canManage = computed(() => can('identity.manage'))
const providers = useQuery(listIdentityProvidersOptions())
const remove = useMutation(deleteIdentityProviderMutation())

const editing = ref<IdentityProviderResponse>()
const formOpen = ref(false)
function openForm(provider?: IdentityProviderResponse) {
  editing.value = provider
  formOpen.value = true
}

const removing = ref<IdentityProviderResponse | null>(null)
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
    { path: { id: provider.id } },
    {
      onSuccess: () => {
        toast.success(t('providers.deleted'))
        removing.value = null
        void providers.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="Fingerprint"
      :title="t('providers.title')"
      :description="t('providers.description')"
    >
      <template v-if="canManage" #actions>
        <Button size="sm" @click="openForm()">
          <Plus data-icon="inline-start" aria-hidden="true" />
          {{ t('providers.new') }}
        </Button>
      </template>
    </PageHeader>

    <ApiFailureAlert
      v-if="providers.isError.value && !providers.data.value"
      :error="providers.error.value"
      retryable
      @retry="providers.refetch()"
    />
    <div v-else-if="providers.isPending.value" class="flex flex-col gap-2">
      <Skeleton v-for="index in 2" :key="index" class="h-12 w-full" />
    </div>
    <div
      v-else-if="!providers.data.value?.length"
      class="text-muted-foreground flex flex-col items-center gap-2 rounded-lg border border-dashed p-10 text-center text-sm"
    >
      <Fingerprint class="size-6" aria-hidden="true" />
      {{ t('providers.empty') }}
    </div>
    <div v-else class="rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{{ t('providers.name') }}</TableHead>
            <TableHead>{{ t('providers.groupRoles') }}</TableHead>
            <TableHead class="w-20"
              ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="provider in providers.data.value" :key="provider.id">
            <TableCell class="max-w-80 align-top whitespace-normal">
              <span class="flex flex-wrap items-center gap-2 font-medium">
                {{ provider.display_name }}
                <Badge v-if="!provider.enabled" variant="outline">{{
                  t('providers.disabled')
                }}</Badge>
                <Badge v-if="provider.create_accounts" variant="secondary">
                  <UserPlus aria-hidden="true" />
                  {{ t('providers.newAccounts') }}
                </Badge>
                <Badge v-if="!provider.has_client_secret" variant="secondary">
                  <KeyRound aria-hidden="true" />
                  {{ t('providers.publicBadge') }}
                </Badge>
              </span>
              <code class="text-muted-foreground block font-mono text-xs">{{ provider.id }}</code>
              <span class="text-muted-foreground block text-xs break-all">
                {{ provider.issuer }}
              </span>
            </TableCell>
            <TableCell class="align-top whitespace-normal">
              <span v-if="provider.group_roles.length" class="flex flex-wrap gap-1">
                <Badge
                  v-for="mapping in provider.group_roles"
                  :key="`${mapping.group}:${mapping.role}`"
                  variant="outline"
                  class="font-mono"
                  >{{ mapping.group }} → {{ mapping.role }}</Badge
                >
              </span>
              <span v-else class="text-muted-foreground">—</span>
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
                  @click="openForm(provider)"
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

    <PasswordSignInCard v-if="providers.data.value?.length" />

    <IdentityProviderFormSheet
      v-model:open="formOpen"
      :provider="editing"
      @saved="providers.refetch()"
    />
    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="t('providers.deleteTitle', { name: removing?.display_name ?? '' })"
      :description="t('providers.deleteDetail')"
      :confirm-label="t('common.delete')"
      destructive
      :busy="remove.isPending.value"
      @confirm="confirmRemove"
    />
  </div>
</template>
