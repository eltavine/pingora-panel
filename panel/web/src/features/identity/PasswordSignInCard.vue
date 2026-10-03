<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { KeyRound, Save, Siren, Users } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { AuditEvent, PasswordSignInMode } from '@/api/generated'
import {
  getSignInPolicyOptions,
  listAuditEventsOptions,
  putSignInPolicyMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ChoiceCards from '@/components/ChoiceCards.vue'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Spinner } from '@/components/ui/spinner'
import { notifyFailure } from '@/lib/configuration'
import { useSession } from '@/lib/session'

const { t, d } = useI18n()
const { can } = useSession()
const canManage = computed(() => can('identity.manage'))
const canAudit = computed(() => can('audit.read'))
const policy = useQuery(getSignInPolicyOptions())
const save = useMutation(putSignInPolicyMutation())
const uses = useQuery({
  ...listAuditEventsOptions({ query: { type: 'identity.break_glass.used', limit: 5 } }),
  enabled: canAudit,
})

const chosen = ref<PasswordSignInMode>('everyone')
watch(
  () => policy.data.value?.password_sign_in,
  (value) => {
    if (value) {
      chosen.value = value
    }
  },
  { immediate: true },
)
const choices = computed(() => [
  { value: 'everyone' as const, label: t('providers.passwordEveryone'), icon: Users },
  { value: 'break_glass_only' as const, label: t('providers.passwordBreakGlass'), icon: Siren },
])

function submit() {
  save.mutate(
    { body: { password_sign_in: chosen.value } },
    {
      onSuccess: () => {
        toast.success(t('providers.policySaved'))
        void policy.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

/** Where a break-glass sign-in came from. */
function origin(event: AuditEvent): string | undefined {
  const attempt = event.data.attempt as { client_address?: string | null } | undefined
  return attempt?.client_address ?? undefined
}
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle class="flex items-center gap-2">
        <KeyRound class="size-4" aria-hidden="true" />
        {{ t('providers.passwordSignIn') }}
      </CardTitle>
      <CardDescription>{{ t('providers.passwordSignInHint') }}</CardDescription>
    </CardHeader>
    <CardContent class="flex flex-col gap-4">
      <Alert v-if="uses.data.value?.items.length" role="status">
        <Siren aria-hidden="true" />
        <AlertTitle>{{ t('providers.breakGlassUsed') }}</AlertTitle>
        <AlertDescription>
          <ul class="flex flex-col gap-0.5">
            <li v-for="event in uses.data.value.items" :key="event.sequence" class="tabular-nums">
              <span class="font-medium">{{ event.actor_id }}</span>
              <template v-if="event.occurred_at">
                · {{ d(new Date(event.occurred_at), 'datetime') }}</template
              >
              <template v-if="origin(event)"> · {{ origin(event) }}</template>
            </li>
          </ul>
        </AlertDescription>
      </Alert>
      <ChoiceCards
        v-if="canManage"
        v-model="chosen"
        :label="t('providers.passwordSignIn')"
        :choices="choices"
      />
      <p v-else class="text-sm">
        {{
          policy.data.value?.password_sign_in === 'break_glass_only'
            ? t('providers.passwordBreakGlass')
            : t('providers.passwordEveryone')
        }}
      </p>
      <p class="text-muted-foreground text-xs">
        {{
          chosen === 'break_glass_only'
            ? t('providers.passwordBreakGlassHint')
            : t('providers.passwordEveryoneHint')
        }}
      </p>
      <div v-if="canManage" class="flex justify-end">
        <Button
          size="sm"
          :disabled="save.isPending.value || chosen === policy.data.value?.password_sign_in"
          @click="submit"
        >
          <Spinner v-if="save.isPending.value" data-icon="inline-start" />
          <Save v-else data-icon="inline-start" aria-hidden="true" />
          {{ t('common.save') }}
        </Button>
      </div>
    </CardContent>
  </Card>
</template>
