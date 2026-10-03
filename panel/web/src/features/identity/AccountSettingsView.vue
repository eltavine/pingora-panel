<script setup lang="ts">
import { computed, reactive, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { KeyRound, LockKeyhole, MonitorSmartphone, Plus, Trash2, UserCog } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { SessionView, TokenView } from '@/api/generated'
import {
  changePasswordMutation,
  endOwnSessionMutation,
  ownSessionsOptions,
  ownTokensOptions,
  revokeOwnTokenMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import FormField from '@/components/FormField.vue'
import PageHeader from '@/components/PageHeader.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { Spinner } from '@/components/ui/spinner'
import { notifyFailure } from '@/lib/configuration'
import { useSession } from '@/lib/session'
import { fieldProblems } from './failures'
import PasswordInput from './PasswordInput.vue'
import SessionTable from './SessionTable.vue'
import TokenCreateSheet from './TokenCreateSheet.vue'
import TokenTable from './TokenTable.vue'

const { t } = useI18n()
const { session } = useSession()
const sessions = useQuery(ownSessionsOptions())
const tokens = useQuery(ownTokensOptions())
const changePassword = useMutation(changePasswordMutation())
const endSession = useMutation(endOwnSessionMutation())
const revokeToken = useMutation(revokeOwnTokenMutation())

const account = computed(() => session.value?.account)
const otherSessions = computed(() => (sessions.data.value ?? []).filter((item) => !item.current))

const password = reactive({ current: '', next: '', confirmation: '' })
const passwordProblems = ref<string[]>([])
const mismatch = computed(
  () => password.confirmation !== '' && password.confirmation !== password.next,
)

function submitPassword() {
  passwordProblems.value = []
  changePassword.mutate(
    { body: { current: password.current, new: password.next } },
    {
      onSuccess: () => {
        toast.success(t('account.passwordChanged'))
        Object.assign(password, { current: '', next: '', confirmation: '' })
        void sessions.refetch()
      },
      onError: (error) => {
        passwordProblems.value = fieldProblems(error, 'password')
        if (!passwordProblems.value.length) {
          notifyFailure(error, t('common.changeFailed'))
        }
      },
    },
  )
}

function end(item: SessionView) {
  endSession.mutate(
    { path: { id: item.id } },
    {
      onSuccess: () => {
        toast.success(t('account.sessionEnded'))
        void sessions.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

const creating = ref(false)
const revoking = ref<TokenView | null>(null)
const revokeOpen = computed({
  get: () => revoking.value !== null,
  set: (open) => {
    if (!open) {
      revoking.value = null
    }
  },
})
function confirmRevoke() {
  const token = revoking.value
  if (!token) {
    return
  }
  revokeToken.mutate(
    { path: { id: token.id } },
    {
      onSuccess: () => {
        toast.success(t('account.tokenRevoked'))
        revoking.value = null
        void tokens.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="UserCog"
      :title="t('account.title')"
      :description="t('account.description')"
    />

    <Card>
      <CardHeader>
        <CardTitle>{{ t('account.profile') }}</CardTitle>
      </CardHeader>
      <CardContent>
        <Skeleton v-if="!account" class="h-20 w-full" />
        <dl v-else class="grid gap-x-6 gap-y-3 text-sm sm:grid-cols-[10rem_1fr]">
          <dt class="text-muted-foreground">{{ t('account.username') }}</dt>
          <dd class="font-mono">{{ account.username }}</dd>
          <template v-if="account.display_name">
            <dt class="text-muted-foreground">{{ t('account.displayName') }}</dt>
            <dd>{{ account.display_name }}</dd>
          </template>
          <dt class="text-muted-foreground">{{ t('account.roles') }}</dt>
          <dd class="flex flex-wrap gap-1">
            <Badge v-for="role in account.roles" :key="role" variant="secondary">{{ role }}</Badge>
          </dd>
          <dt class="text-muted-foreground">{{ t('account.permissions') }}</dt>
          <dd class="flex flex-wrap gap-1">
            <Badge
              v-for="permission in session?.permissions ?? []"
              :key="permission"
              variant="outline"
              class="font-mono"
              >{{ permission }}</Badge
            >
          </dd>
        </dl>
      </CardContent>
    </Card>

    <Card>
      <form @submit.prevent="submitPassword">
        <CardHeader>
          <CardTitle class="flex items-center gap-2">
            <LockKeyhole class="size-4" aria-hidden="true" />
            {{ t('account.changePassword') }}
          </CardTitle>
          <CardDescription>{{ t('auth.passwordHint') }}</CardDescription>
        </CardHeader>
        <CardContent class="grid gap-4 sm:grid-cols-3">
          <FormField id="password-current" :label="t('account.currentPassword')">
            <PasswordInput
              id="password-current"
              v-model="password.current"
              autocomplete="current-password"
            />
          </FormField>
          <FormField id="password-new" :label="t('account.newPassword')">
            <PasswordInput
              id="password-new"
              v-model="password.next"
              autocomplete="new-password"
              :invalid="passwordProblems.length > 0"
            />
          </FormField>
          <FormField id="password-confirm" :label="t('account.confirmPassword')">
            <PasswordInput
              id="password-confirm"
              v-model="password.confirmation"
              autocomplete="new-password"
              :invalid="mismatch"
            />
          </FormField>
          <ul
            v-if="passwordProblems.length || mismatch"
            class="text-destructive flex flex-col gap-0.5 text-xs sm:col-span-3"
            role="alert"
          >
            <li v-if="mismatch">{{ t('auth.mismatch') }}</li>
            <li v-for="item in passwordProblems" :key="item">{{ item }}</li>
          </ul>
        </CardContent>
        <CardFooter class="mt-4 justify-end">
          <Button
            type="submit"
            :disabled="
              changePassword.isPending.value || mismatch || !password.current || !password.next
            "
          >
            <Spinner v-if="changePassword.isPending.value" data-icon="inline-start" />
            <LockKeyhole v-else data-icon="inline-start" aria-hidden="true" />
            {{ t('account.changePassword') }}
          </Button>
        </CardFooter>
      </form>
    </Card>

    <Card>
      <CardHeader>
        <CardTitle class="flex items-center gap-2">
          <MonitorSmartphone class="size-4" aria-hidden="true" />
          {{ t('account.sessions') }}
        </CardTitle>
        <CardDescription>{{ t('account.sessionsDetail') }}</CardDescription>
      </CardHeader>
      <CardContent>
        <Skeleton v-if="sessions.isPending.value" class="h-24 w-full" />
        <SessionTable
          v-else-if="sessions.data.value?.length"
          :sessions="sessions.data.value"
          :busy="endSession.isPending.value"
          @end="end"
        />
        <p
          v-if="sessions.data.value && !otherSessions.length"
          class="text-muted-foreground mt-3 text-sm"
        >
          {{ t('account.noSessions') }}
        </p>
      </CardContent>
    </Card>

    <Card>
      <CardHeader class="flex flex-row items-start justify-between gap-4">
        <div class="flex flex-col gap-1.5">
          <CardTitle class="flex items-center gap-2">
            <KeyRound class="size-4" aria-hidden="true" />
            {{ t('account.tokens') }}
          </CardTitle>
          <CardDescription>{{ t('account.tokensDetail') }}</CardDescription>
        </div>
        <Button size="sm" variant="outline" @click="creating = true">
          <Plus data-icon="inline-start" aria-hidden="true" />
          {{ t('account.newToken') }}
        </Button>
      </CardHeader>
      <CardContent>
        <Skeleton v-if="tokens.isPending.value" class="h-24 w-full" />
        <TokenTable
          v-else-if="tokens.data.value?.length"
          :tokens="tokens.data.value"
          :busy="revokeToken.isPending.value"
          @revoke="revoking = $event"
        />
        <p v-else class="text-muted-foreground text-sm">{{ t('account.noTokens') }}</p>
      </CardContent>
    </Card>

    <TokenCreateSheet
      v-model:open="creating"
      :permissions="session?.permissions ?? []"
      @created="tokens.refetch()"
    />
    <ConfirmDialog
      v-model:open="revokeOpen"
      :icon="Trash2"
      :title="t('account.revokeTitle', { name: revoking?.name ?? '' })"
      :description="t('account.revokeDetail')"
      :confirm-label="t('account.revoke')"
      destructive
      :busy="revokeToken.isPending.value"
      @confirm="confirmRevoke"
    />
  </div>
</template>
