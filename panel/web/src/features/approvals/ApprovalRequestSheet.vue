<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Check, Undo2, UserCheck, X } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { ApprovalRequest } from '@/api/generated'
import FormField from '@/components/FormField.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Spinner } from '@/components/ui/spinner'
import { Textarea } from '@/components/ui/textarea'
import { useSession } from '@/lib/session'
import { actions, type Decision, stateVariant, validApprovals } from './presentation'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ request?: ApprovalRequest; busy: boolean }>()
const emit = defineEmits<{ decide: [decision: Decision, reason?: string] }>()

const { t, d } = useI18n()
const { session, can } = useSession()
const reason = ref('')
watch(open, (isOpen) => {
  if (isOpen) {
    reason.value = ''
  }
})
const allowed = computed(() =>
  props.request ? actions(props.request, session.value?.account.username, can) : undefined,
)
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
      <template v-if="request">
        <SheetHeader>
          <SheetTitle class="flex flex-wrap items-center gap-2">
            {{ t('approvals.request') }}
            <Badge :variant="stateVariant(request.state)">{{
              t(`approvals.states.${request.state}`)
            }}</Badge>
          </SheetTitle>
          <SheetDescription class="font-mono text-xs break-all">{{ request.id }}</SheetDescription>
        </SheetHeader>
        <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 px-4 text-sm">
          <dt class="text-muted-foreground">{{ t('approvals.requestedBy') }}</dt>
          <dd>{{ request.requested_by }} · {{ d(new Date(request.requested_at), 'datetime') }}</dd>
          <dt class="text-muted-foreground">{{ t('approvals.risk') }}</dt>
          <dd>{{ t(`approvals.risks.${request.risk}`) }}</dd>
          <dt class="text-muted-foreground">{{ t('approvals.policies') }}</dt>
          <dd class="flex flex-wrap gap-1">
            <Badge
              v-for="policy in request.policies"
              :key="policy.id"
              variant="outline"
              class="font-mono"
              >{{ policy.id }} v{{ policy.version }}</Badge
            >
          </dd>
          <dt class="text-muted-foreground">{{ t('approvals.approvals') }}</dt>
          <dd>{{ validApprovals(request) }}/{{ request.required }}</dd>
          <dt class="text-muted-foreground">{{ t('approvals.expires') }}</dt>
          <dd>{{ d(new Date(request.expires_at), 'datetime') }}</dd>
          <template v-if="request.note">
            <dt class="text-muted-foreground">{{ t('approvals.note') }}</dt>
            <dd>{{ request.note }}</dd>
          </template>
          <template v-if="request.reason">
            <dt class="text-muted-foreground">{{ t('approvals.reason') }}</dt>
            <dd>{{ request.reason }}</dd>
          </template>
        </dl>
        <section class="flex flex-col gap-2 px-4">
          <h3 class="text-sm font-medium">{{ t('approvals.changes') }}</h3>
          <ul class="flex flex-col gap-1 text-sm">
            <li v-for="change in request.changes" :key="change.resource" class="flex gap-2">
              <Badge variant="outline">{{ t(`approvals.change.${change.change}`) }}</Badge>
              <code class="font-mono text-xs break-all">{{ change.resource }}</code>
            </li>
          </ul>
        </section>
        <section v-if="request.approvals.length" class="flex flex-col gap-2 px-4">
          <h3 class="text-sm font-medium">{{ t('approvals.approvedBy') }}</h3>
          <ul class="flex flex-col gap-1 text-sm">
            <li v-for="approval in request.approvals" :key="approval.approver" class="flex gap-2">
              <UserCheck class="size-4" aria-hidden="true" />
              <span :class="{ 'line-through': approval.revoked_at }">{{ approval.approver }}</span>
              <span class="text-muted-foreground text-xs">
                {{
                  approval.revoked_at
                    ? t('approvals.revokedAt', { at: d(new Date(approval.revoked_at), 'datetime') })
                    : t('approvals.validUntil', {
                        at: d(new Date(approval.valid_until), 'datetime'),
                      })
                }}
              </span>
            </li>
          </ul>
        </section>
        <FormField
          v-if="allowed?.reject"
          id="approval-reason"
          class="px-4"
          :label="t('approvals.rejectReason')"
        >
          <Textarea id="approval-reason" v-model="reason" rows="2" maxlength="512" />
        </FormField>
        <SheetFooter class="flex-row flex-wrap justify-end gap-2">
          <Button
            v-if="allowed?.withdraw"
            variant="outline"
            :disabled="busy"
            @click="emit('decide', 'withdraw')"
          >
            <Undo2 data-icon="inline-start" aria-hidden="true" />
            {{ t('approvals.withdraw') }}
          </Button>
          <Button
            v-if="allowed?.revoke"
            variant="outline"
            :disabled="busy"
            @click="emit('decide', 'revoke')"
          >
            <Undo2 data-icon="inline-start" aria-hidden="true" />
            {{ t('approvals.revoke') }}
          </Button>
          <Button
            v-if="allowed?.reject"
            variant="outline"
            :disabled="busy"
            @click="emit('decide', 'reject', reason.trim() || undefined)"
          >
            <X data-icon="inline-start" aria-hidden="true" />
            {{ t('approvals.reject') }}
          </Button>
          <Button v-if="allowed?.approve" :disabled="busy" @click="emit('decide', 'approve')">
            <Spinner v-if="busy" data-icon="inline-start" />
            <Check v-else data-icon="inline-start" aria-hidden="true" />
            {{ t('approvals.approve') }}
          </Button>
        </SheetFooter>
      </template>
    </SheetContent>
  </Sheet>
</template>
