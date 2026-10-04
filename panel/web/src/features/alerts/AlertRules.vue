<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { BellRing, History, Pencil, Plus, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { AlertRuleView } from '@/api/generated'
import {
  deleteAlertRuleMutation,
  listAlertRulesOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Button } from '@/components/ui/button'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Skeleton } from '@/components/ui/skeleton'
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
import AlertRuleFormSheet from './AlertRuleFormSheet.vue'
import { formatMeasure, stateKey, toneOf } from './presentation'

/** Rules are read again this often, so states stay current. */
const REFRESH_INTERVAL_MS = 15_000

const emit = defineEmits<{ notifications: [rule: string] }>()
const { t, d, locale } = useI18n()
const { can } = useSession()
const canManage = computed(() => can('alerts.manage'))
const rules = useQuery({ ...listAlertRulesOptions(), refetchInterval: REFRESH_INTERVAL_MS })
const remove = useMutation(deleteAlertRuleMutation())

const editing = ref<AlertRuleView>()
const formOpen = ref(false)
function openForm(rule?: AlertRuleView) {
  editing.value = rule
  formOpen.value = true
}

const removing = ref<AlertRuleView | null>(null)
const removeOpen = computed({
  get: () => removing.value !== null,
  set: (open) => {
    if (!open) {
      removing.value = null
    }
  },
})
function confirmRemove() {
  const rule = removing.value
  if (!rule) {
    return
  }
  remove.mutate(
    { path: { id: rule.id }, headers: changeHeaders(rule.etag) },
    {
      onSuccess: () => {
        toast.success(t('alerts.ruleDeleted', { name: rule.spec.name }))
        removing.value = null
        void rules.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

function condition(rule: AlertRuleView): string {
  return t(`alerts.comparisons.${rule.spec.comparison}`, {
    measure: t(`alerts.measures.${rule.spec.measure}`),
    threshold: formatMeasure(rule.spec.measure, rule.spec.threshold, locale.value),
  })
}

function scope(rule: AlertRuleView): string {
  const spec = rule.spec
  if (spec.site && spec.route) {
    return `${spec.site} / ${spec.route}`
  }
  return spec.site ?? spec.upstream ?? t('alerts.everything')
}
</script>

<template>
  <div class="flex flex-col gap-4">
    <div v-if="canManage" class="flex justify-end">
      <Button size="sm" @click="openForm()">
        <Plus data-icon="inline-start" aria-hidden="true" />
        {{ t('alerts.newRule') }}
      </Button>
    </div>
    <ApiFailureAlert
      v-if="rules.isError.value && !rules.data.value"
      :error="rules.error.value"
      retryable
      @retry="rules.refetch()"
    />
    <div v-else-if="rules.isPending.value" class="flex flex-col gap-2">
      <Skeleton v-for="index in 3" :key="index" class="h-11 w-full" />
    </div>
    <Empty v-else-if="(rules.data.value ?? []).length === 0" class="border">
      <EmptyHeader>
        <EmptyMedia variant="icon"><BellRing aria-hidden="true" /></EmptyMedia>
        <EmptyTitle>{{ t('alerts.noRulesTitle') }}</EmptyTitle>
        <EmptyDescription>{{ t('alerts.noRulesDetail') }}</EmptyDescription>
      </EmptyHeader>
    </Empty>
    <div v-else class="rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{{ t('alerts.columns.rule') }}</TableHead>
            <TableHead>{{ t('alerts.columns.state') }}</TableHead>
            <TableHead class="hidden md:table-cell">{{ t('alerts.columns.condition') }}</TableHead>
            <TableHead class="hidden lg:table-cell">{{ t('alerts.columns.scope') }}</TableHead>
            <TableHead class="hidden sm:table-cell">{{ t('alerts.columns.value') }}</TableHead>
            <TableHead class="w-0"
              ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="rule in rules.data.value" :key="rule.id">
            <TableCell class="min-w-0">
              <div class="flex flex-col">
                <span class="font-medium">{{ rule.spec.name }}</span>
                <span class="text-muted-foreground font-mono text-xs">{{ rule.id }}</span>
              </div>
            </TableCell>
            <TableCell class="whitespace-nowrap">
              <div class="flex flex-col gap-0.5">
                <StatusIndicator :tone="toneOf(rule)" :label="t(stateKey(rule))" />
                <time
                  v-if="rule.since && rule.spec.enabled"
                  :datetime="rule.since"
                  class="text-muted-foreground text-xs"
                  >{{ t('alerts.since', { time: d(new Date(rule.since), 'datetime') }) }}</time
                >
              </div>
            </TableCell>
            <TableCell class="hidden md:table-cell">
              <div class="flex flex-col">
                <span>{{ condition(rule) }}</span>
                <span v-if="rule.spec.pending_seconds" class="text-muted-foreground text-xs">
                  {{
                    t(
                      'alerts.forMinutes',
                      { minutes: Math.round(rule.spec.pending_seconds / 60) },
                      Math.round(rule.spec.pending_seconds / 60),
                    )
                  }}
                </span>
              </div>
            </TableCell>
            <TableCell class="hidden lg:table-cell">{{ scope(rule) }}</TableCell>
            <TableCell class="hidden tabular-nums sm:table-cell">
              <span v-if="rule.evaluation_error" class="text-destructive text-xs">
                {{ t('alerts.unreadable') }}
              </span>
              <span v-else>{{ formatMeasure(rule.spec.measure, rule.value, locale) }}</span>
            </TableCell>
            <TableCell>
              <div class="flex justify-end gap-1">
                <Button
                  variant="ghost"
                  size="icon-sm"
                  :aria-label="t('alerts.showNotifications', { name: rule.spec.name })"
                  @click="emit('notifications', rule.id)"
                >
                  <History aria-hidden="true" />
                </Button>
                <template v-if="canManage">
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    :aria-label="t('alerts.editRule', { name: rule.spec.name })"
                    @click="openForm(rule)"
                  >
                    <Pencil aria-hidden="true" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    :aria-label="t('alerts.deleteRule', { name: rule.spec.name })"
                    @click="removing = rule"
                  >
                    <Trash2 aria-hidden="true" />
                  </Button>
                </template>
              </div>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <AlertRuleFormSheet v-model:open="formOpen" :rule="editing" @saved="rules.refetch()" />
    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="t('alerts.deleteRuleTitle')"
      :description="t('alerts.deleteRuleDescription', { name: removing?.spec.name ?? '' })"
      :confirm-label="t('common.delete')"
      :busy="remove.isPending.value"
      destructive
      @confirm="confirmRemove"
    />
  </div>
</template>
