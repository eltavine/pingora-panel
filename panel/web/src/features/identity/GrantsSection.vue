<script setup lang="ts">
import { computed, reactive, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Plus, ShieldPlus, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { AccountView, GrantScopeBody, GrantView } from '@/api/generated'
import {
  createGrantMutation,
  deleteGrantMutation,
  listGrantsOptions,
  listRolesOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import WindowsField from '@/components/WindowsField.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { notifyFailure } from '@/lib/configuration'
import { describeWindow, windowInput, type WindowForm } from '@/lib/windows'

const props = defineProps<{ account: AccountView; canManage: boolean }>()

const { t, d } = useI18n()
const grants = useQuery(computed(() => listGrantsOptions({ path: { id: props.account.id } })))
const roles = useQuery(listRolesOptions())
const create = useMutation(createGrantMutation())
const remove = useMutation(deleteGrantMutation())

const adding = ref(false)
const blank = () => ({
  role: '',
  scope: 'site_group' as 'everything' | 'site_group' | 'site',
  target: '',
  until: '',
  networks: '',
  windows: [] as WindowForm[],
})
const form = reactive(blank())
const complete = computed(
  () => form.role && (form.scope === 'everything' || form.target.trim().length > 0),
)

function scopeOf(): GrantScopeBody {
  if (form.scope === 'site_group') {
    return { kind: 'site_group', group: form.target.trim() }
  }
  if (form.scope === 'site') {
    return { kind: 'site', site: form.target.trim() }
  }
  return { kind: 'everything' }
}

function submit() {
  create.mutate(
    {
      path: { id: props.account.id },
      body: {
        role: form.role,
        scope: scopeOf(),
        conditions: {
          not_after: form.until ? new Date(form.until).toISOString() : null,
          networks: form.networks.split(/[\s,]+/).filter(Boolean),
          windows: form.windows.map((window) => windowInput(window)),
        },
      },
    },
    {
      onSuccess: () => {
        toast.success(t('grants.created'))
        Object.assign(form, blank())
        adding.value = false
        void grants.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

function revoke(grant: GrantView) {
  remove.mutate(
    { path: { id: props.account.id, grant: grant.id } },
    {
      onSuccess: () => {
        toast.success(t('grants.revoked'))
        void grants.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

function scopeText(grant: GrantView): string {
  switch (grant.scope.kind) {
    case 'site_group':
      return t('grants.groupScope', { group: grant.scope.group })
    case 'site':
      return t('grants.siteScope', { site: grant.scope.site })
    default:
      return t('grants.everything')
  }
}

function conditionText(grant: GrantView): string[] {
  const parts: string[] = []
  const conditions = grant.conditions
  if (conditions.not_after) {
    parts.push(t('grants.until', { at: d(new Date(conditions.not_after), 'datetime') }))
  }
  if (conditions.networks?.length) {
    parts.push(t('grants.from', { networks: conditions.networks.join(', ') }))
  }
  for (const window of conditions.windows ?? []) {
    parts.push(describeWindow(window, (day) => t(`windows.days.${day}`)))
  }
  return parts
}
</script>

<template>
  <section class="flex flex-col gap-3">
    <div class="flex items-center justify-between gap-2">
      <h3 class="flex items-center gap-2 text-sm font-medium">
        <ShieldPlus class="size-4" aria-hidden="true" />
        {{ t('grants.title') }}
      </h3>
      <Button v-if="canManage && !adding" size="sm" variant="outline" @click="adding = true">
        <Plus data-icon="inline-start" aria-hidden="true" />
        {{ t('grants.add') }}
      </Button>
    </div>
    <p class="text-muted-foreground text-xs">{{ t('grants.description') }}</p>
    <ul v-if="grants.data.value?.length" class="divide-y rounded-md border">
      <li
        v-for="grant in grants.data.value"
        :key="grant.id"
        class="flex items-start justify-between gap-3 p-3"
      >
        <div class="flex min-w-0 flex-col gap-1">
          <span class="flex flex-wrap items-center gap-2 text-sm">
            <Badge variant="secondary">{{ grant.role }}</Badge>
            {{ scopeText(grant) }}
          </span>
          <span class="flex flex-wrap gap-1">
            <Badge v-for="part in conditionText(grant)" :key="part" variant="outline">{{
              part
            }}</Badge>
            <Badge v-if="!conditionText(grant).length" variant="outline">{{
              t('grants.always')
            }}</Badge>
          </span>
        </div>
        <Button
          v-if="canManage"
          variant="ghost"
          size="icon-sm"
          :aria-label="t('grants.revoke')"
          :title="t('grants.revoke')"
          :disabled="remove.isPending.value"
          @click="revoke(grant)"
        >
          <Trash2 aria-hidden="true" />
        </Button>
      </li>
    </ul>
    <p v-else-if="!adding" class="text-muted-foreground text-sm">{{ t('grants.empty') }}</p>

    <form v-if="adding" class="flex flex-col gap-4 rounded-md border p-3" @submit.prevent="submit">
      <div class="grid gap-4 sm:grid-cols-2">
        <FormField id="grant-role" :label="t('grants.role')">
          <Select v-model="form.role">
            <SelectTrigger id="grant-role" class="w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem v-for="role in roles.data.value ?? []" :key="role.id" :value="role.id">
                {{ role.name }}
              </SelectItem>
            </SelectContent>
          </Select>
        </FormField>
        <FormField id="grant-scope" :label="t('grants.scope')">
          <Select v-model="form.scope">
            <SelectTrigger id="grant-scope" class="w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="site_group">{{ t('grants.siteGroup') }}</SelectItem>
              <SelectItem value="site">{{ t('grants.site') }}</SelectItem>
              <SelectItem value="everything">{{ t('grants.everything') }}</SelectItem>
            </SelectContent>
          </Select>
        </FormField>
      </div>
      <FormField
        v-if="form.scope !== 'everything'"
        id="grant-target"
        :label="form.scope === 'site' ? t('grants.siteId') : t('grants.groupName')"
        :hint="t('grants.scopeHint')"
      >
        <Input id="grant-target" v-model="form.target" spellcheck="false" required />
      </FormField>
      <div class="grid gap-4 sm:grid-cols-2">
        <FormField id="grant-until" :label="t('grants.untilLabel')">
          <Input id="grant-until" v-model="form.until" type="datetime-local" />
        </FormField>
        <FormField
          id="grant-networks"
          :label="t('grants.networks')"
          :hint="t('grants.networksHint')"
        >
          <Input id="grant-networks" v-model="form.networks" spellcheck="false" />
        </FormField>
      </div>
      <WindowsField
        id="grant-windows"
        v-model="form.windows"
        :legend="t('grants.windows')"
        :hint="t('grants.windowsHint')"
      />
      <div class="flex justify-end gap-2">
        <Button type="button" variant="ghost" @click="adding = false">{{
          t('common.cancel')
        }}</Button>
        <Button type="submit" :disabled="create.isPending.value || !complete">
          <ShieldPlus data-icon="inline-start" aria-hidden="true" />
          {{ t('grants.give') }}
        </Button>
      </div>
    </form>
  </section>
</template>
