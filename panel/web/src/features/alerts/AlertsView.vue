<script setup lang="ts">
import { computed } from 'vue'
import { BellRing, History, ListChecks, Webhook } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import PageHeader from '@/components/PageHeader.vue'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import AlertChannels from './AlertChannels.vue'
import AlertNotifications from './AlertNotifications.vue'
import AlertRules from './AlertRules.vue'

const TABS = ['rules', 'channels', 'notifications'] as const
type Tab = (typeof TABS)[number]

const { t } = useI18n()
const route = useRoute()
const router = useRouter()

function queried(name: string): string {
  const value = route.query[name]
  return typeof value === 'string' ? value : ''
}

/** The tab and the rule whose notifications show live in the address, so
 * notifications can link to a rule. */
const tab = computed({
  get: (): Tab => {
    const value = queried('tab')
    return (TABS as readonly string[]).includes(value) ? (value as Tab) : 'rules'
  },
  set: (value: string) =>
    void router.replace({
      query: { ...route.query, tab: value === 'rules' ? undefined : value },
    }),
})
const rule = computed({
  get: () => queried('rule'),
  set: (value: string) =>
    void router.replace({ query: { ...route.query, rule: value || undefined } }),
})

function notificationsOf(id: string) {
  void router.replace({ query: { ...route.query, tab: 'notifications', rule: id } })
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="BellRing"
      :title="t('alerts.title')"
      :description="t('alerts.description')"
    />
    <Tabs v-model="tab">
      <TabsList>
        <TabsTrigger value="rules">
          <ListChecks aria-hidden="true" />
          {{ t('alerts.rules') }}
        </TabsTrigger>
        <TabsTrigger value="channels">
          <Webhook aria-hidden="true" />
          {{ t('alerts.channels') }}
        </TabsTrigger>
        <TabsTrigger value="notifications">
          <History aria-hidden="true" />
          {{ t('alerts.notifications') }}
        </TabsTrigger>
      </TabsList>
      <TabsContent value="rules" class="pt-2">
        <AlertRules @notifications="notificationsOf" />
      </TabsContent>
      <TabsContent value="channels" class="pt-2">
        <AlertChannels />
      </TabsContent>
      <TabsContent value="notifications" class="pt-2">
        <AlertNotifications v-model:rule="rule" />
      </TabsContent>
    </Tabs>
  </div>
</template>
