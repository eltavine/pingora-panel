<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Lock, Pencil, Plus, Shield, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { RoleView } from '@/api/generated'
import { deleteRoleMutation, listRolesOptions } from '@/api/generated/@tanstack/vue-query.gen'
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
import RoleFormSheet from './RoleFormSheet.vue'

const { t } = useI18n()
const { can } = useSession()
const canManage = computed(() => can('identity.manage'))
const roles = useQuery(listRolesOptions())
const remove = useMutation(deleteRoleMutation())

const editing = ref<RoleView>()
const formOpen = ref(false)
function openForm(role?: RoleView) {
  editing.value = role
  formOpen.value = true
}

const removing = ref<RoleView | null>(null)
const removeOpen = computed({
  get: () => removing.value !== null,
  set: (open) => {
    if (!open) {
      removing.value = null
    }
  },
})
function confirmRemove() {
  const role = removing.value
  if (!role) {
    return
  }
  remove.mutate(
    { path: { id: role.id } },
    {
      onSuccess: () => {
        toast.success(t('roles.deleted'))
        removing.value = null
        void roles.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader :icon="Shield" :title="t('roles.title')" :description="t('roles.description')">
      <template v-if="canManage" #actions>
        <Button size="sm" @click="openForm()">
          <Plus data-icon="inline-start" aria-hidden="true" />
          {{ t('roles.new') }}
        </Button>
      </template>
    </PageHeader>

    <ApiFailureAlert
      v-if="roles.isError.value && !roles.data.value"
      :error="roles.error.value"
      retryable
      @retry="roles.refetch()"
    />
    <div v-else-if="roles.isPending.value" class="flex flex-col gap-2">
      <Skeleton v-for="index in 3" :key="index" class="h-12 w-full" />
    </div>
    <div v-else class="rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{{ t('roles.name') }}</TableHead>
            <TableHead>{{ t('roles.permissions') }}</TableHead>
            <TableHead class="w-20"
              ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="role in roles.data.value" :key="role.id">
            <TableCell class="max-w-72 align-top">
              <span class="flex items-center gap-2 font-medium">
                {{ role.name }}
                <Badge v-if="role.built_in" variant="secondary">
                  <Lock aria-hidden="true" />
                  {{ t('roles.builtIn') }}
                </Badge>
              </span>
              <code class="text-muted-foreground block font-mono text-xs">{{ role.id }}</code>
              <span v-if="role.description" class="text-muted-foreground block text-xs">
                {{ role.description }}
              </span>
            </TableCell>
            <TableCell class="align-top">
              <span class="flex flex-wrap gap-1">
                <Badge
                  v-for="permission in role.permissions"
                  :key="permission"
                  variant="outline"
                  class="font-mono"
                  >{{ permission }}</Badge
                >
              </span>
            </TableCell>
            <TableCell class="align-top">
              <span v-if="canManage && !role.built_in" class="flex justify-end gap-1">
                <Button
                  variant="ghost"
                  size="icon-sm"
                  :aria-label="t('common.edit')"
                  :title="t('common.edit')"
                  @click="openForm(role)"
                >
                  <Pencil aria-hidden="true" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  :aria-label="t('common.delete')"
                  :title="t('common.delete')"
                  @click="removing = role"
                >
                  <Trash2 aria-hidden="true" />
                </Button>
              </span>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <RoleFormSheet v-model:open="formOpen" :role="editing" @saved="roles.refetch()" />
    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="t('roles.deleteTitle', { name: removing?.name ?? '' })"
      :description="t('roles.deleteDetail')"
      :confirm-label="t('common.delete')"
      destructive
      :busy="remove.isPending.value"
      @confirm="confirmRemove"
    />
  </div>
</template>
