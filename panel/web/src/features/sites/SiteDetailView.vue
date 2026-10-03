<script setup lang="ts">
import { computed, ref } from 'vue'
import { useQuery, useQueryClient } from '@tanstack/vue-query'
import {
  ArrowLeft,
  Copy,
  Download,
  Globe,
  Pause,
  Pencil,
  Play,
  Route as RouteIcon,
  Settings2,
  Star,
  Trash2,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import type { SiteView } from '@/api/generated'
import { getSiteOptions, getSiteQueryKey } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle, CardAction } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { useRefreshConfiguration } from '@/lib/configuration'
import DomainsPanel from './DomainsPanel.vue'
import RoutesPanel from './RoutesPanel.vue'
import SiteFormSheet from './SiteFormSheet.vue'
import { useSiteCommands } from './useSiteCommands'
import { kindIcons, statusTones } from './presentation'

const TABS = ['settings', 'domains', 'routes'] as const
type Tab = (typeof TABS)[number]

const props = defineProps<{ id: string }>()

const { t, d } = useI18n()
const route = useRoute()
const router = useRouter()
const client = useQueryClient()
const refresh = useRefreshConfiguration()
const commands = useSiteCommands()
const site = useQuery(computed(() => getSiteOptions({ path: { id: props.id } })))

const tab = computed<Tab>({
  get: () => {
    const value = route.query.tab
    return TABS.find((item) => item === value) ?? 'settings'
  },
  set: (value) => void router.replace({ query: { ...route.query, tab: value } }),
})

const editing = ref(false)
const deleting = ref(false)

function saved(updated: SiteView) {
  client.setQueryData(getSiteQueryKey({ path: { id: props.id } }), updated)
  void refresh()
}

const facts = computed(() => {
  const value = site.data.value
  if (!value) {
    return []
  }
  const none = t('state.none')
  return [
    { label: t('sites.columns.kind'), value: t(`sites.kind.${value.kind}`) },
    {
      label: t('sites.form.httpsRedirect'),
      value: value.https_redirect ? t('common.yes') : t('common.no'),
    },
    {
      label: t('sites.form.wwwRedirect'),
      value:
        value.www_redirect === 'add_www'
          ? t('sites.form.wwwAdd')
          : value.www_redirect === 'remove_www'
            ? t('sites.form.wwwRemove')
            : t('sites.form.wwwNone'),
    },
    { label: t('sites.form.listeners'), value: value.listener_ids?.join(', ') || t('common.all') },
    { label: t('sites.form.tlsProfile'), value: value.tls_profile_id ?? none },
    { label: t('sites.form.group'), value: value.group ?? none },
    { label: t('sites.form.note'), value: value.note ?? none },
    { label: t('sites.columns.updated'), value: d(new Date(value.updated_at), 'datetime') },
  ]
})
</script>

<template>
  <div class="flex flex-col gap-6">
    <div>
      <Button variant="ghost" size="sm" as-child>
        <RouterLink to="/sites">
          <ArrowLeft data-icon="inline-start" aria-hidden="true" />
          {{ t('common.back') }}
        </RouterLink>
      </Button>
    </div>

    <ApiFailureAlert
      v-if="site.isError.value && !site.data.value"
      :error="site.error.value"
      retryable
      @retry="site.refetch()"
    />
    <Skeleton v-else-if="!site.data.value" class="h-40 w-full" />

    <template v-else>
      <PageHeader
        :icon="kindIcons[site.data.value.kind]"
        :title="site.data.value.name"
        :description="t(`sites.kind.${site.data.value.kind}`)"
      >
        <template #actions>
          <StatusIndicator
            :tone="statusTones[site.data.value.status]"
            :label="t(`sites.status.${site.data.value.status}`)"
          />
          <Button
            variant="outline"
            size="icon-sm"
            :aria-pressed="site.data.value.favorite ?? false"
            :aria-label="site.data.value.favorite ? t('sites.unfavorite') : t('sites.favorite')"
            @click="commands.setFavorite(site.data.value, !site.data.value.favorite)"
          >
            <Star :class="{ 'fill-current': site.data.value.favorite }" aria-hidden="true" />
          </Button>
          <Button
            variant="outline"
            size="sm"
            :disabled="commands.busy.value"
            @click="commands.setEnabled(site.data.value, !(site.data.value.enabled ?? true))"
          >
            <component
              :is="(site.data.value.enabled ?? true) ? Pause : Play"
              data-icon="inline-start"
              aria-hidden="true"
            />
            {{ (site.data.value.enabled ?? true) ? t('common.disable') : t('common.enable') }}
          </Button>
          <Button
            variant="outline"
            size="sm"
            @click="commands.clone(site.data.value, (clone) => router.push(`/sites/${clone.id}`))"
          >
            <Copy data-icon="inline-start" aria-hidden="true" />
            {{ t('sites.clone') }}
          </Button>
          <Button variant="outline" size="sm" @click="commands.exportSites([site.data.value.id])">
            <Download data-icon="inline-start" aria-hidden="true" />
            {{ t('common.export') }}
          </Button>
          <Button variant="destructive" size="sm" @click="deleting = true">
            <Trash2 data-icon="inline-start" aria-hidden="true" />
            {{ t('common.delete') }}
          </Button>
        </template>
      </PageHeader>

      <Tabs v-model="tab">
        <TabsList>
          <TabsTrigger value="settings">
            <Settings2 aria-hidden="true" />
            {{ t('sites.tabs.settings') }}
          </TabsTrigger>
          <TabsTrigger value="domains">
            <Globe aria-hidden="true" />
            {{ t('sites.tabs.domains') }}
            <Badge variant="secondary">{{ site.data.value.domains?.length ?? 0 }}</Badge>
          </TabsTrigger>
          <TabsTrigger value="routes">
            <RouteIcon aria-hidden="true" />
            {{ t('sites.tabs.routes') }}
            <Badge variant="secondary">{{ site.data.value.routes?.length ?? 0 }}</Badge>
          </TabsTrigger>
        </TabsList>

        <TabsContent value="settings" class="pt-4">
          <Card>
            <CardHeader>
              <CardTitle>{{ t('sites.tabs.settings') }}</CardTitle>
              <CardAction>
                <Button variant="outline" size="sm" @click="editing = true">
                  <Pencil data-icon="inline-start" aria-hidden="true" />
                  {{ t('common.edit') }}
                </Button>
              </CardAction>
            </CardHeader>
            <CardContent>
              <dl class="grid gap-x-6 gap-y-4 sm:grid-cols-2">
                <div v-for="fact in facts" :key="fact.label" class="flex min-w-0 flex-col gap-1">
                  <dt class="text-muted-foreground text-xs">{{ fact.label }}</dt>
                  <dd class="truncate text-sm">{{ fact.value }}</dd>
                </div>
                <div class="flex min-w-0 flex-col gap-1">
                  <dt class="text-muted-foreground text-xs">{{ t('sites.form.tags') }}</dt>
                  <dd class="flex flex-wrap gap-1">
                    <Badge v-for="tag in site.data.value.tags ?? []" :key="tag" variant="outline">
                      {{ tag }}
                    </Badge>
                    <span v-if="!site.data.value.tags?.length" class="text-sm">{{
                      t('state.none')
                    }}</span>
                  </dd>
                </div>
              </dl>
            </CardContent>
          </Card>
        </TabsContent>
        <TabsContent value="domains" class="pt-4">
          <DomainsPanel :site="site.data.value" @saved="saved" />
        </TabsContent>
        <TabsContent value="routes" class="pt-4">
          <RoutesPanel :site="site.data.value" />
        </TabsContent>
      </Tabs>

      <SiteFormSheet v-model:open="editing" :site="site.data.value" @saved="saved" />

      <ConfirmDialog
        v-model:open="deleting"
        :icon="Trash2"
        :title="t('sites.confirmDeleteTitle', { count: 1 })"
        :description="t('sites.confirmDeleteDetail')"
        :confirm-label="t('common.delete')"
        destructive
        :busy="commands.busy.value"
        @confirm="commands.remove(site.data.value, false, () => router.push('/sites'))"
      />
    </template>
  </div>
</template>
