<script setup lang="ts">
import { computed, ref } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { CircleAlert, Fingerprint, GitCommitHorizontal, ReceiptText, Search } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { receiptOptions } from '@/api/generated/@tanstack/vue-query.gen'
import type { IdempotencyReceiptResponse, ReceiptOutcomeResponse } from '@/api/generated'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import CopyValue from '@/components/CopyValue.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatusIndicator, { type StatusTone } from '@/components/StatusIndicator.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Skeleton } from '@/components/ui/skeleton'

const { t } = useI18n()
const draft = ref('')
const key = ref('')

const receipt = useQuery(
  computed(() => ({ ...receiptOptions({ path: { key: key.value } }), enabled: key.value !== '' })),
)

const outcome = computed<ReceiptOutcomeResponse | null>(() => {
  const data = receipt.data.value
  return data && 'outcome' in data ? (data as IdempotencyReceiptResponse).outcome : null
})

const presentation: Record<ReceiptOutcomeResponse['status'], { tone: StatusTone; label: string }> =
  {
    succeeded: { tone: 'positive', label: 'receipts.succeeded' },
    rejected: { tone: 'negative', label: 'receipts.rejected' },
    failed_before_commit: { tone: 'negative', label: 'receipts.failedBeforeCommit' },
    pending_reconciliation: { tone: 'warning', label: 'receipts.pendingReconciliation' },
    unknown: { tone: 'warning', label: 'receipts.unknown' },
  }

function lookUp() {
  key.value = draft.value.trim()
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="ReceiptText"
      :title="t('receipts.title')"
      :description="t('receipts.description')"
    />

    <form class="flex max-w-xl flex-col gap-2 sm:flex-row sm:items-end" @submit.prevent="lookUp">
      <div class="grid flex-1 gap-2">
        <Label for="receipt-key">{{ t('receipts.key') }}</Label>
        <Input id="receipt-key" v-model="draft" class="font-mono" autocomplete="off" />
      </div>
      <Button type="submit" :disabled="draft.trim() === ''">
        <Search data-icon="inline-start" aria-hidden="true" />
        {{ t('receipts.lookup') }}
      </Button>
    </form>

    <Empty v-if="key === ''" class="border border-dashed">
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <ReceiptText aria-hidden="true" />
        </EmptyMedia>
        <EmptyTitle>{{ t('receipts.emptyTitle') }}</EmptyTitle>
        <EmptyDescription>{{ t('receipts.emptyDetail') }}</EmptyDescription>
      </EmptyHeader>
    </Empty>

    <Skeleton
      v-else-if="receipt.isPending.value"
      class="h-32 rounded-xl"
      :aria-label="t('state.loading')"
    />

    <ApiFailureAlert
      v-else-if="receipt.isError.value"
      :error="receipt.error.value"
      retryable
      @retry="receipt.refetch()"
    />

    <Card v-else-if="receipt.data.value">
      <CardHeader>
        <CardTitle>
          <StatusIndicator
            v-if="outcome"
            :tone="presentation[outcome.status].tone"
            :label="t(presentation[outcome.status].label)"
          />
          <StatusIndicator v-else tone="pending" :label="t('receipts.pending')" />
        </CardTitle>
      </CardHeader>
      <CardContent v-if="outcome?.status === 'succeeded'">
        <dl class="grid gap-3 text-sm sm:grid-cols-[auto_1fr]">
          <dt class="text-muted-foreground flex items-center gap-2">
            <GitCommitHorizontal class="size-4" aria-hidden="true" />{{ t('publish.revision') }}
          </dt>
          <dd class="font-mono tabular-nums">{{ outcome.revision_id }}</dd>
          <dt class="text-muted-foreground flex items-center gap-2">
            <Fingerprint class="size-4" aria-hidden="true" />{{ t('publish.contentHash') }}
          </dt>
          <dd class="min-w-0"><CopyValue :value="outcome.content_hash" /></dd>
        </dl>
      </CardContent>
      <CardContent v-else-if="outcome?.status === 'rejected'">
        <ul class="flex flex-col gap-2">
          <li
            v-for="(item, index) in outcome.diagnostics"
            :key="index"
            class="flex items-start gap-2 text-sm"
          >
            <CircleAlert class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
            <span
              ><code class="font-mono text-xs">{{ item.code }}</code> {{ item.message }}</span
            >
          </li>
        </ul>
      </CardContent>
    </Card>
  </div>
</template>
