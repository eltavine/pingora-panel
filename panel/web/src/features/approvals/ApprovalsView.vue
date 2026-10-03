<script setup lang="ts">
import { ref } from 'vue'
import { Inbox, ShieldCheck, Stamp } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import PageHeader from '@/components/PageHeader.vue'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import ApprovalPolicies from './ApprovalPolicies.vue'
import ApprovalRequests from './ApprovalRequests.vue'

const { t } = useI18n()
const tab = ref('requests')
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="Stamp"
      :title="t('approvals.title')"
      :description="t('approvals.description')"
    />
    <Tabs v-model="tab">
      <TabsList>
        <TabsTrigger value="requests">
          <Inbox aria-hidden="true" />
          {{ t('approvals.requests') }}
        </TabsTrigger>
        <TabsTrigger value="policies">
          <ShieldCheck aria-hidden="true" />
          {{ t('approvals.policiesTab') }}
        </TabsTrigger>
      </TabsList>
      <TabsContent value="requests" class="pt-2">
        <ApprovalRequests />
      </TabsContent>
      <TabsContent value="policies" class="pt-2">
        <ApprovalPolicies />
      </TabsContent>
    </Tabs>
  </div>
</template>
