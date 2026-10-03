<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { FileBadge, KeyRound, Lock, LockOpen, Pencil, Plus, Radio, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { ListenerView, TlsProfileView } from '@/api/generated'
import {
  deleteListenerMutation,
  deleteTlsProfileMutation,
  listListenersOptions,
  listSitesOptions,
  listTlsProfilesOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import PageHeader from '@/components/PageHeader.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
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
import { changeHeaders, notifyFailure, useRefreshConfiguration } from '@/lib/configuration'
import ListenerFormSheet from './ListenerFormSheet.vue'
import TlsProfileFormSheet from './TlsProfileFormSheet.vue'

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const listeners = useQuery(listListenersOptions())
const profiles = useQuery(listTlsProfilesOptions())
const sites = useQuery(listSitesOptions({ query: { limit: 500 } }))
const removeListener = useMutation(deleteListenerMutation())
const removeProfile = useMutation(deleteTlsProfileMutation())

const siteNames = computed(
  () => new Map((sites.data.value?.items ?? []).map((site) => [site.id, site.name])),
)
const listenerIds = computed(() => (listeners.data.value ?? []).map((item) => item.id))
const profileIds = computed(() => (profiles.data.value ?? []).map((item) => item.id))

function protocols(listener: ListenerView) {
  const value = listener.protocols ?? { http1: true, http2: false, http3: false }
  return [
    value.http1 && t('listeners.http1'),
    value.http2 && t('listeners.http2'),
    value.http3 && t('listeners.http3'),
  ].filter((item): item is string => Boolean(item))
}

const editingListener = ref<ListenerView | undefined>()
const listenerOpen = ref(false)
function openListener(listener?: ListenerView) {
  editingListener.value = listener
  listenerOpen.value = true
}
const editingProfile = ref<TlsProfileView | undefined>()
const profileOpen = ref(false)
function openProfile(profile?: TlsProfileView) {
  editingProfile.value = profile
  profileOpen.value = true
}

type Removal = { kind: 'listener'; item: ListenerView } | { kind: 'profile'; item: TlsProfileView }
const removing = ref<Removal | null>(null)
const removeOpen = computed({
  get: () => removing.value !== null,
  set: (open) => {
    if (!open) {
      removing.value = null
    }
  },
})
function confirmRemove() {
  const target = removing.value
  if (!target) {
    return
  }
  const options = {
    onSuccess: () => {
      toast.success(
        target.kind === 'listener' ? t('listeners.deleted') : t('listeners.profiles.deleted'),
      )
      removing.value = null
      void refresh()
    },
    onError: (error: unknown) => notifyFailure(error, t('common.changeFailed')),
  }
  const request = { path: { id: target.item.id }, headers: changeHeaders(target.item.etag) }
  if (target.kind === 'listener') {
    removeListener.mutate(request, options)
  } else {
    removeProfile.mutate(request, options)
  }
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="Radio"
      :title="t('listeners.title')"
      :description="t('listeners.description')"
    />

    <Card>
      <CardHeader>
        <CardTitle class="flex items-center gap-2">
          <Radio class="size-4" aria-hidden="true" />
          {{ t('listeners.listTitle') }}
        </CardTitle>
        <CardDescription>{{ t('listeners.defaultSiteHint') }}</CardDescription>
        <CardAction>
          <Button size="sm" @click="openListener()">
            <Plus data-icon="inline-start" aria-hidden="true" />
            {{ t('listeners.new') }}
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent>
        <ApiFailureAlert
          v-if="listeners.isError.value && !listeners.data.value"
          :error="listeners.error.value"
          retryable
          @retry="listeners.refetch()"
        />
        <Skeleton v-else-if="listeners.isPending.value" class="h-24 w-full" />
        <Empty v-else-if="(listeners.data.value ?? []).length === 0" class="border">
          <EmptyHeader>
            <EmptyMedia variant="icon"><Radio aria-hidden="true" /></EmptyMedia>
            <EmptyTitle>{{ t('listeners.emptyTitle') }}</EmptyTitle>
            <EmptyDescription>{{ t('listeners.emptyDetail') }}</EmptyDescription>
          </EmptyHeader>
        </Empty>
        <div v-else class="rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{{ t('listeners.id') }}</TableHead>
                <TableHead>{{ t('listeners.address') }}</TableHead>
                <TableHead>{{ t('listeners.protocols') }}</TableHead>
                <TableHead>{{ t('listeners.tlsProfile') }}</TableHead>
                <TableHead>{{ t('listeners.defaultSite') }}</TableHead>
                <TableHead class="w-20"
                  ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
                >
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="listener in listeners.data.value" :key="listener.id">
                <TableCell class="font-mono text-xs">{{ listener.id }}</TableCell>
                <TableCell>
                  <span class="inline-flex items-center gap-1.5 font-mono text-xs">
                    <Lock v-if="listener.tls_profile_id" class="size-3.5" aria-hidden="true" />
                    <LockOpen v-else class="size-3.5" aria-hidden="true" />
                    {{ listener.address }}
                  </span>
                </TableCell>
                <TableCell>
                  <div class="flex flex-wrap gap-1">
                    <Badge
                      v-for="protocol in protocols(listener)"
                      :key="protocol"
                      variant="outline"
                    >
                      {{ protocol }}
                    </Badge>
                    <Badge v-if="listener.reuse_port" variant="secondary">{{
                      t('listeners.reusePort')
                    }}</Badge>
                    <Badge v-if="listener.ipv6_only" variant="secondary">{{
                      t('listeners.ipv6Only')
                    }}</Badge>
                  </div>
                </TableCell>
                <TableCell class="font-mono text-xs">{{
                  listener.tls_profile_id ?? t('listeners.plain')
                }}</TableCell>
                <TableCell>
                  <RouterLink
                    v-if="listener.default_site_id"
                    :to="`/sites/${listener.default_site_id}`"
                    class="text-sm hover:underline"
                  >
                    {{ siteNames.get(listener.default_site_id) ?? listener.default_site_id }}
                  </RouterLink>
                  <span v-else class="text-muted-foreground text-xs">421</span>
                </TableCell>
                <TableCell>
                  <div class="flex justify-end gap-1">
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      :aria-label="t('common.edit')"
                      @click="openListener(listener)"
                    >
                      <Pencil aria-hidden="true" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      :aria-label="t('common.delete')"
                      @click="removing = { kind: 'listener', item: listener }"
                    >
                      <Trash2 aria-hidden="true" />
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
      </CardContent>
    </Card>

    <Card>
      <CardHeader>
        <CardTitle class="flex items-center gap-2">
          <KeyRound class="size-4" aria-hidden="true" />
          {{ t('listeners.profiles.title') }}
        </CardTitle>
        <CardDescription>{{ t('listeners.profiles.emptyDetail') }}</CardDescription>
        <CardAction>
          <Button size="sm" @click="openProfile()">
            <Plus data-icon="inline-start" aria-hidden="true" />
            {{ t('listeners.profiles.new') }}
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent>
        <Skeleton v-if="profiles.isPending.value" class="h-24 w-full" />
        <Empty v-else-if="(profiles.data.value ?? []).length === 0" class="border">
          <EmptyHeader>
            <EmptyMedia variant="icon"><KeyRound aria-hidden="true" /></EmptyMedia>
            <EmptyTitle>{{ t('listeners.profiles.emptyTitle') }}</EmptyTitle>
            <EmptyDescription>{{ t('listeners.profiles.emptyDetail') }}</EmptyDescription>
          </EmptyHeader>
        </Empty>
        <div v-else class="rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{{ t('listeners.id') }}</TableHead>
                <TableHead>{{ t('listeners.profiles.inventoryCertificate') }}</TableHead>
                <TableHead>{{ t('listeners.profiles.minProtocol') }}</TableHead>
                <TableHead>{{ t('listeners.profiles.alpn') }}</TableHead>
                <TableHead class="w-20"
                  ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
                >
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="profile in profiles.data.value" :key="profile.id">
                <TableCell class="font-mono text-xs">{{ profile.id }}</TableCell>
                <TableCell class="font-mono text-xs">
                  <span v-if="profile.certificate_id" class="flex items-center gap-1.5">
                    <FileBadge class="size-3.5" aria-hidden="true" />
                    {{ profile.certificate_id }}
                  </span>
                  <span v-else
                    >{{ profile.certificate_secret_id }} · {{ profile.private_key_secret_id }}</span
                  >
                </TableCell>
                <TableCell>{{ profile.min_protocol }}</TableCell>
                <TableCell>
                  <div class="flex flex-wrap gap-1">
                    <Badge v-for="protocol in profile.alpn" :key="protocol" variant="outline">{{
                      protocol
                    }}</Badge>
                  </div>
                </TableCell>
                <TableCell>
                  <div class="flex justify-end gap-1">
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      :aria-label="t('common.edit')"
                      @click="openProfile(profile)"
                    >
                      <Pencil aria-hidden="true" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      :aria-label="t('common.delete')"
                      @click="removing = { kind: 'profile', item: profile }"
                    >
                      <Trash2 aria-hidden="true" />
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
      </CardContent>
    </Card>

    <ListenerFormSheet
      v-model:open="listenerOpen"
      :listener="editingListener"
      :taken="listenerIds"
    />
    <TlsProfileFormSheet v-model:open="profileOpen" :profile="editingProfile" :taken="profileIds" />

    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="
        removing?.kind === 'profile'
          ? t('listeners.profiles.confirmDeleteTitle', { id: removing.item.id })
          : t('listeners.confirmDeleteTitle', { id: removing?.item.id ?? '' })
      "
      :description="
        removing?.kind === 'profile'
          ? t('listeners.profiles.confirmDeleteDetail')
          : t('listeners.confirmDeleteDetail')
      "
      :confirm-label="t('common.delete')"
      destructive
      :busy="removeListener.isPending.value || removeProfile.isPending.value"
      @confirm="confirmRemove"
    />
  </div>
</template>
