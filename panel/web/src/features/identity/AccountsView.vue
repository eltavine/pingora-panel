<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import {
  Bot,
  Ellipsis,
  KeyRound,
  LockKeyhole,
  LockKeyholeOpen,
  Pencil,
  Plus,
  Siren,
  UserCheck,
  UserX,
  Users,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { AccountView } from '@/api/generated'
import {
  listAccountsOptions,
  listRolesOptions,
  updateAccountMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import PageHeader from '@/components/PageHeader.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
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
import AccountAccessSheet from './AccountAccessSheet.vue'
import AccountFormSheet from './AccountFormSheet.vue'
import PasswordResetSheet from './PasswordResetSheet.vue'
import TokenCreateSheet from './TokenCreateSheet.vue'
import { accountState } from './presentation'

const { t, d } = useI18n()
const { can, session } = useSession()
const canManage = computed(() => can('identity.manage'))
const accounts = useQuery(listAccountsOptions())
const update = useMutation(updateAccountMutation())

const editing = ref<AccountView>()
const formOpen = ref(false)
function openForm(account?: AccountView) {
  editing.value = account
  formOpen.value = true
}

const selected = ref<AccountView>()
const roleCatalog = useQuery(listRolesOptions())
const issueOpen = ref(false)
function openIssue(account: AccountView) {
  selected.value = account
  issueOpen.value = true
}
/** The permissions an account's roles grant. */
function permissionsOf(account: AccountView): string[] {
  const granted = (roleCatalog.data.value ?? [])
    .filter((role) => account.roles.includes(role.id))
    .flatMap((role) => role.permissions)
  return [...new Set(granted)].sort()
}
const resetOpen = ref(false)
const accessOpen = ref(false)
function openReset(account: AccountView) {
  selected.value = account
  resetOpen.value = true
}
function openAccess(account: AccountView) {
  selected.value = account
  accessOpen.value = true
}

function change(
  account: AccountView,
  body: { disabled?: boolean; unlock?: boolean; break_glass?: boolean },
  done: string,
) {
  update.mutate(
    { path: { id: account.id }, body },
    {
      onSuccess: () => {
        toast.success(done)
        disabling.value = null
        void accounts.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

const disabling = ref<AccountView | null>(null)
const disableOpen = computed({
  get: () => disabling.value !== null,
  set: (open) => {
    if (!open) {
      disabling.value = null
    }
  },
})
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader :icon="Users" :title="t('accounts.title')" :description="t('accounts.description')">
      <template v-if="canManage" #actions>
        <Button size="sm" @click="openForm()">
          <Plus data-icon="inline-start" aria-hidden="true" />
          {{ t('accounts.new') }}
        </Button>
      </template>
    </PageHeader>

    <ApiFailureAlert
      v-if="accounts.isError.value && !accounts.data.value"
      :error="accounts.error.value"
      retryable
      @retry="accounts.refetch()"
    />
    <div v-else-if="accounts.isPending.value" class="flex flex-col gap-2">
      <Skeleton v-for="index in 3" :key="index" class="h-12 w-full" />
    </div>

    <div v-else class="rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{{ t('accounts.username') }}</TableHead>
            <TableHead>{{ t('accounts.roles') }}</TableHead>
            <TableHead>{{ t('accounts.state') }}</TableHead>
            <TableHead>{{ t('accounts.lastLogin') }}</TableHead>
            <TableHead class="w-12"
              ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="account in accounts.data.value" :key="account.id">
            <TableCell class="max-w-64">
              <span class="block truncate font-mono font-medium">{{ account.username }}</span>
              <span
                v-if="account.display_name"
                class="text-muted-foreground block truncate text-xs"
              >
                {{ account.display_name }}
              </span>
            </TableCell>
            <TableCell>
              <span class="flex flex-wrap gap-1">
                <Badge v-for="role in account.roles" :key="role" variant="secondary">{{
                  role
                }}</Badge>
              </span>
            </TableCell>
            <TableCell>
              <Badge :variant="accountState(account) === 'active' ? 'outline' : 'destructive'">
                <LockKeyhole v-if="accountState(account) === 'locked'" aria-hidden="true" />
                <UserX v-else-if="accountState(account) === 'disabled'" aria-hidden="true" />
                <UserCheck v-else aria-hidden="true" />
                {{ t(`accounts.${accountState(account)}`) }}
              </Badge>
              <Badge v-if="account.service" variant="secondary" class="ml-1">
                <Bot aria-hidden="true" />
                {{ t('accounts.service') }}
              </Badge>
              <Badge v-if="account.break_glass" variant="secondary" class="ml-1">
                <Siren aria-hidden="true" />
                {{ t('accounts.breakGlass') }}
              </Badge>
            </TableCell>
            <TableCell class="text-muted-foreground text-xs tabular-nums">
              {{
                account.last_login_at
                  ? d(new Date(account.last_login_at), 'datetime')
                  : t('accounts.neverLoggedIn')
              }}
            </TableCell>
            <TableCell>
              <DropdownMenu>
                <DropdownMenuTrigger as-child>
                  <Button variant="ghost" size="icon-sm" :aria-label="t('common.actions')">
                    <Ellipsis aria-hidden="true" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem @select="openAccess(account)">
                    <KeyRound aria-hidden="true" />
                    {{ t('accounts.access') }}
                  </DropdownMenuItem>
                  <template v-if="canManage">
                    <DropdownMenuItem @select="openForm(account)">
                      <Pencil aria-hidden="true" />
                      {{ t('common.edit') }}
                    </DropdownMenuItem>
                    <DropdownMenuItem v-if="account.service" @select="openIssue(account)">
                      <KeyRound aria-hidden="true" />
                      {{ t('accounts.issueToken') }}
                    </DropdownMenuItem>
                    <DropdownMenuItem v-else @select="openReset(account)">
                      <LockKeyhole aria-hidden="true" />
                      {{ t('accounts.resetPassword') }}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      v-if="account.locked"
                      @select="change(account, { unlock: true }, t('accounts.unlocked'))"
                    >
                      <LockKeyholeOpen aria-hidden="true" />
                      {{ t('accounts.unlock') }}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      v-if="!account.service"
                      @select="
                        change(
                          account,
                          { break_glass: !account.break_glass },
                          t('accounts.updated'),
                        )
                      "
                    >
                      <Siren aria-hidden="true" />
                      {{
                        account.break_glass
                          ? t('accounts.unmarkBreakGlass')
                          : t('accounts.markBreakGlass')
                      }}
                    </DropdownMenuItem>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem
                      v-if="account.disabled"
                      @select="change(account, { disabled: false }, t('accounts.updated'))"
                    >
                      <UserCheck aria-hidden="true" />
                      {{ t('accounts.enable') }}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      v-else
                      :disabled="account.id === session?.account.id"
                      variant="destructive"
                      @select="disabling = account"
                    >
                      <UserX aria-hidden="true" />
                      {{ t('accounts.disable') }}
                    </DropdownMenuItem>
                  </template>
                </DropdownMenuContent>
              </DropdownMenu>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <AccountFormSheet v-model:open="formOpen" :account="editing" @saved="accounts.refetch()" />
    <PasswordResetSheet
      v-if="selected"
      v-model:open="resetOpen"
      :account="selected"
      @saved="accounts.refetch()"
    />
    <TokenCreateSheet
      v-if="selected"
      v-model:open="issueOpen"
      :account="selected"
      :permissions="permissionsOf(selected)"
    />
    <AccountAccessSheet
      v-if="selected"
      v-model:open="accessOpen"
      :account="selected"
      :can-manage="canManage"
    />
    <ConfirmDialog
      v-model:open="disableOpen"
      :icon="UserX"
      :title="t('accounts.disableTitle', { name: disabling?.username ?? '' })"
      :description="t('accounts.disableDetail')"
      :confirm-label="t('accounts.disable')"
      destructive
      :busy="update.isPending.value"
      @confirm="disabling && change(disabling, { disabled: true }, t('accounts.updated'))"
    />
  </div>
</template>
