<script setup lang="ts">
import { computed } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { KeyRound, MonitorSmartphone } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { AccountView, SessionView, TokenView } from '@/api/generated'
import {
  accountSessionsOptions,
  accountTokensOptions,
  endAccountSessionMutation,
  revokeAccountTokenMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { notifyFailure } from '@/lib/configuration'
import SessionTable from './SessionTable.vue'
import TokenTable from './TokenTable.vue'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ account: AccountView; canManage: boolean }>()

const { t } = useI18n()
const path = computed(() => ({ path: { id: props.account.id } }))
const sessions = useQuery(
  computed(() => ({ ...accountSessionsOptions(path.value), enabled: open.value })),
)
const tokens = useQuery(
  computed(() => ({ ...accountTokensOptions(path.value), enabled: open.value })),
)
const endSession = useMutation(endAccountSessionMutation())
const revokeToken = useMutation(revokeAccountTokenMutation())

function end(session: SessionView) {
  endSession.mutate(
    { path: { id: props.account.id, session: session.id } },
    {
      onSuccess: () => {
        toast.success(t('account.sessionEnded'))
        void sessions.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

function revoke(token: TokenView) {
  revokeToken.mutate(
    { path: { id: props.account.id, token: token.id } },
    {
      onSuccess: () => {
        toast.success(t('account.tokenRevoked'))
        void tokens.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-3xl">
      <SheetHeader>
        <SheetTitle>{{ t('accounts.accessTitle', { name: account.username }) }}</SheetTitle>
        <SheetDescription>{{ t('account.sessionsDetail') }}</SheetDescription>
      </SheetHeader>
      <div class="flex flex-col gap-6 px-4 pb-6">
        <section class="flex flex-col gap-3">
          <h3 class="flex items-center gap-2 text-sm font-medium">
            <MonitorSmartphone class="size-4" aria-hidden="true" />
            {{ t('account.sessions') }}
          </h3>
          <Skeleton v-if="sessions.isPending.value" class="h-20 w-full" />
          <SessionTable
            v-else-if="sessions.data.value?.length"
            :sessions="sessions.data.value"
            :busy="!canManage || endSession.isPending.value"
            @end="end"
          />
          <p v-else class="text-muted-foreground text-sm">{{ t('account.noSessions') }}</p>
        </section>
        <section class="flex flex-col gap-3">
          <h3 class="flex items-center gap-2 text-sm font-medium">
            <KeyRound class="size-4" aria-hidden="true" />
            {{ t('account.tokens') }}
          </h3>
          <Skeleton v-if="tokens.isPending.value" class="h-20 w-full" />
          <TokenTable
            v-else-if="tokens.data.value?.length"
            :tokens="tokens.data.value"
            :busy="!canManage || revokeToken.isPending.value"
            @revoke="revoke"
          />
          <p v-else class="text-muted-foreground text-sm">{{ t('account.noTokens') }}</p>
        </section>
      </div>
    </SheetContent>
  </Sheet>
</template>
