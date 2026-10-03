<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Bot, Plus, Save, X } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { WorkloadIdentityResponse } from '@/api/generated'
import {
  listAccountsOptions,
  putWorkloadIdentityMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import SwitchField from '@/components/SwitchField.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Spinner } from '@/components/ui/spinner'
import { notifyFailure } from '@/lib/configuration'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ trust?: WorkloadIdentityResponse }>()
const emit = defineEmits<{ saved: [] }>()

const { t } = useI18n()
const accounts = useQuery(listAccountsOptions())
const services = computed(() => (accounts.data.value ?? []).filter((account) => account.service))
const save = useMutation(putWorkloadIdentityMutation())

const blank = () => ({
  id: '',
  accountId: '',
  issuer: 'https://token.actions.githubusercontent.com',
  audience: 'pingora-panel',
  subject: '',
  claims: [] as { name: string; value: string }[],
  sessionMinutes: 15,
  enabled: true,
})
const form = reactive(blank())
watch(open, (isOpen) => {
  if (!isOpen) {
    return
  }
  const trust = props.trust
  Object.assign(
    form,
    trust
      ? {
          id: trust.id,
          accountId: trust.account_id,
          issuer: trust.issuer,
          audience: trust.audience,
          subject: trust.subject,
          claims: Object.entries(trust.claims).map(([name, value]) => ({ name, value })),
          sessionMinutes: trust.session_minutes,
          enabled: trust.enabled,
        }
      : blank(),
  )
})
const complete = computed(
  () =>
    form.id.trim() &&
    form.accountId &&
    form.issuer.trim() &&
    form.audience.trim() &&
    form.subject.trim(),
)

function submit() {
  const claims = Object.fromEntries(
    form.claims
      .map((claim) => [claim.name.trim(), claim.value.trim()])
      .filter(([name, value]) => name && value),
  )
  save.mutate(
    {
      path: { id: form.id.trim() },
      body: {
        account_id: form.accountId,
        issuer: form.issuer.trim(),
        audience: form.audience.trim(),
        subject: form.subject.trim(),
        claims,
        session_minutes: Number(form.sessionMinutes),
        enabled: form.enabled,
      },
    },
    {
      onSuccess: () => {
        toast.success(t('workloads.saved', { id: form.id.trim() }))
        open.value = false
        emit('saved')
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>{{ trust ? t('workloads.edit') : t('workloads.new') }}</SheetTitle>
          <SheetDescription>{{ trust?.id ?? t('workloads.description') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-5 px-4">
          <FormField v-if="!trust" id="workload-id" :label="t('workloads.id')">
            <Input
              id="workload-id"
              v-model="form.id"
              autocomplete="off"
              autocapitalize="none"
              spellcheck="false"
              pattern="[A-Za-z0-9._\-]{1,64}"
              required
            />
          </FormField>
          <FormField
            id="workload-account"
            :label="t('workloads.account')"
            :hint="t('workloads.accountHint')"
          >
            <Select v-model="form.accountId">
              <SelectTrigger id="workload-account" class="w-full">
                <SelectValue :placeholder="t('workloads.chooseAccount')" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem v-for="account in services" :key="account.id" :value="account.id">
                  {{ account.username }}
                </SelectItem>
              </SelectContent>
            </Select>
          </FormField>
          <FormField
            id="workload-issuer"
            :label="t('workloads.issuer')"
            :hint="t('workloads.issuerHint')"
          >
            <Input
              id="workload-issuer"
              v-model="form.issuer"
              type="url"
              spellcheck="false"
              required
            />
          </FormField>
          <FormField id="workload-audience" :label="t('workloads.audience')">
            <Input id="workload-audience" v-model="form.audience" spellcheck="false" required />
          </FormField>
          <FormField
            id="workload-subject"
            :label="t('workloads.subject')"
            :hint="t('workloads.subjectHint')"
          >
            <Input id="workload-subject" v-model="form.subject" spellcheck="false" required />
          </FormField>
          <fieldset class="flex flex-col gap-2">
            <legend class="mb-1 text-sm font-medium">{{ t('workloads.claims') }}</legend>
            <p class="text-muted-foreground text-xs">{{ t('workloads.claimsHint') }}</p>
            <div v-for="(claim, index) in form.claims" :key="index" class="flex items-center gap-2">
              <Input
                v-model="claim.name"
                :aria-label="t('workloads.claimName')"
                :placeholder="t('workloads.claimName')"
                class="min-w-0 flex-1"
                spellcheck="false"
              />
              <Input
                v-model="claim.value"
                :aria-label="t('workloads.claimValue')"
                :placeholder="t('workloads.claimValue')"
                class="min-w-0 flex-1"
                spellcheck="false"
              />
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                :aria-label="t('workloads.removeClaim')"
                :title="t('workloads.removeClaim')"
                @click="form.claims.splice(index, 1)"
              >
                <X aria-hidden="true" />
              </Button>
            </div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              class="self-start"
              @click="form.claims.push({ name: '', value: '' })"
            >
              <Plus data-icon="inline-start" aria-hidden="true" />
              {{ t('workloads.addClaim') }}
            </Button>
          </fieldset>
          <FormField id="workload-minutes" :label="t('workloads.sessionMinutes')">
            <Input
              id="workload-minutes"
              v-model.number="form.sessionMinutes"
              type="number"
              min="5"
              max="60"
              required
            />
          </FormField>
          <SwitchField
            id="workload-enabled"
            v-model="form.enabled"
            :label="t('workloads.enabled')"
          />
        </div>
        <SheetFooter>
          <Button type="submit" :disabled="save.isPending.value || !complete">
            <Spinner v-if="save.isPending.value" data-icon="inline-start" />
            <Save v-else-if="trust" data-icon="inline-start" aria-hidden="true" />
            <Bot v-else data-icon="inline-start" aria-hidden="true" />
            {{ trust ? t('common.save') : t('common.create') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
