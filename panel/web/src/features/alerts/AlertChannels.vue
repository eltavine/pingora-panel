<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { KeyRound, Plus, Send, Trash2, Webhook } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { AlertChannelSecretView, AlertChannelView } from '@/api/generated'
import {
  deleteAlertChannelMutation,
  listAlertChannelsOptions,
  testAlertChannelMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import { Button } from '@/components/ui/button'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Skeleton } from '@/components/ui/skeleton'
import { Spinner } from '@/components/ui/spinner'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { changeHeaders, notifyFailure, plainHeaders } from '@/lib/configuration'
import { useSession } from '@/lib/session'
import ChannelFormSheet from './ChannelFormSheet.vue'
import ChannelSecretDialog from './ChannelSecretDialog.vue'

const { t } = useI18n()
const { can } = useSession()
const canManage = computed(() => can('alerts.manage'))
const channels = useQuery(listAlertChannelsOptions())
const remove = useMutation(deleteAlertChannelMutation())
const test = useMutation(testAlertChannelMutation())

const formOpen = ref(false)
const rotating = ref<AlertChannelView>()
function openForm(channel?: AlertChannelView) {
  rotating.value = channel
  formOpen.value = true
}
const secret = ref<AlertChannelSecretView>()
function shown(created: AlertChannelSecretView) {
  if (created.secret) {
    secret.value = created
  } else {
    toast.success(t('alerts.pluginChannelCreated', { id: created.channel.id }))
  }
  void channels.refetch()
}

const testing = ref<string>()
function send(channel: AlertChannelView) {
  testing.value = channel.id
  test.mutate(
    { path: { id: channel.id }, headers: plainHeaders() },
    {
      onSuccess: (outcome) => {
        if (outcome.delivered) {
          toast.success(
            outcome.status == null
              ? t('alerts.testTaken', { id: channel.id })
              : t('alerts.testDelivered', { id: channel.id, status: outcome.status }),
          )
        } else {
          toast.error(t('alerts.testFailed', { id: channel.id }), {
            description: outcome.failure ?? undefined,
          })
        }
      },
      onError: (error) => notifyFailure(error, t('alerts.testFailed', { id: channel.id })),
      onSettled: () => (testing.value = undefined),
    },
  )
}

const removing = ref<AlertChannelView | null>(null)
const removeOpen = computed({
  get: () => removing.value !== null,
  set: (open) => {
    if (!open) {
      removing.value = null
    }
  },
})
function confirmRemove() {
  const channel = removing.value
  if (!channel) {
    return
  }
  remove.mutate(
    { path: { id: channel.id }, headers: changeHeaders(channel.etag) },
    {
      onSuccess: () => {
        toast.success(t('alerts.channelDeleted', { id: channel.id }))
        removing.value = null
        void channels.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <div class="flex flex-col gap-4">
    <div v-if="canManage" class="flex justify-end">
      <Button size="sm" @click="openForm()">
        <Plus data-icon="inline-start" aria-hidden="true" />
        {{ t('alerts.newChannel') }}
      </Button>
    </div>
    <ApiFailureAlert
      v-if="channels.isError.value && !channels.data.value"
      :error="channels.error.value"
      retryable
      @retry="channels.refetch()"
    />
    <div v-else-if="channels.isPending.value" class="flex flex-col gap-2">
      <Skeleton v-for="index in 2" :key="index" class="h-11 w-full" />
    </div>
    <Empty v-else-if="(channels.data.value ?? []).length === 0" class="border">
      <EmptyHeader>
        <EmptyMedia variant="icon"><Webhook aria-hidden="true" /></EmptyMedia>
        <EmptyTitle>{{ t('alerts.noChannelsTitle') }}</EmptyTitle>
        <EmptyDescription>{{ t('alerts.noChannelsDetail') }}</EmptyDescription>
      </EmptyHeader>
    </Empty>
    <div v-else class="rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{{ t('alerts.columns.channel') }}</TableHead>
            <TableHead class="hidden sm:table-cell">{{ t('alerts.columns.kind') }}</TableHead>
            <TableHead>{{ t('alerts.columns.target') }}</TableHead>
            <TableHead class="w-0"
              ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="channel in channels.data.value" :key="channel.id">
            <TableCell class="font-mono text-xs font-medium">{{ channel.id }}</TableCell>
            <TableCell class="hidden sm:table-cell">
              {{ t(`alerts.kinds.${channel.kind}`) }}
            </TableCell>
            <TableCell class="max-w-64 truncate font-mono text-xs" :title="channel.target">
              {{ channel.target }}
            </TableCell>
            <TableCell>
              <div v-if="canManage" class="flex justify-end gap-1">
                <Button
                  variant="ghost"
                  size="icon-sm"
                  :disabled="testing === channel.id"
                  :aria-label="t('alerts.testChannel', { id: channel.id })"
                  @click="send(channel)"
                >
                  <Spinner v-if="testing === channel.id" />
                  <Send v-else aria-hidden="true" />
                </Button>
                <Button
                  v-if="channel.kind === 'webhook'"
                  variant="ghost"
                  size="icon-sm"
                  :aria-label="t('alerts.rotateChannel', { id: channel.id })"
                  @click="openForm(channel)"
                >
                  <KeyRound aria-hidden="true" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  :aria-label="t('alerts.deleteChannel', { id: channel.id })"
                  @click="removing = channel"
                >
                  <Trash2 aria-hidden="true" />
                </Button>
              </div>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <ChannelFormSheet v-model:open="formOpen" :channel="rotating" @secret="shown" />
    <ChannelSecretDialog v-model:secret="secret" />
    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="t('alerts.deleteChannelTitle')"
      :description="t('alerts.deleteChannelDescription', { id: removing?.id ?? '' })"
      :confirm-label="t('common.delete')"
      :busy="remove.isPending.value"
      destructive
      @confirm="confirmRemove"
    />
  </div>
</template>
