<script setup lang="ts">
import { computed } from 'vue'
import { useClipboard } from '@vueuse/core'
import { Check, Copy, Link2, Logs } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { LogRecordItem } from '@/api/generated'
import CopyValue from '@/components/CopyValue.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { summaryOf, toneOf } from './presentation'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ record?: LogRecordItem }>()
const emit = defineEmits<{ request: [requestId: string] }>()

const { t, d } = useI18n()
const { copy, copied, isSupported } = useClipboard({ copiedDuring: 1500 })

const facts = computed(() => {
  const record = props.record
  if (!record) {
    return []
  }
  return [
    { label: t('logs.columns.site'), value: record.site },
    { label: t('logs.columns.route'), value: record.route },
    { label: t('logs.columns.status'), value: record.status?.toString() },
    { label: t('logs.method'), value: record.method },
    { label: t('logs.columns.path'), value: record.path },
    { label: t('logs.columns.client'), value: record.client },
  ].filter((fact) => fact.value)
})

const fields = computed(() =>
  Object.entries(props.record?.fields ?? {}).sort(([left], [right]) => left.localeCompare(right)),
)
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-2xl">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2">
          <Logs class="size-5" aria-hidden="true" />
          <time v-if="record" :datetime="record.time">{{
            d(new Date(record.time), 'precise')
          }}</time>
        </SheetTitle>
        <SheetDescription v-if="record" class="flex flex-col gap-1">
          <StatusIndicator
            :tone="toneOf(record)"
            :label="record.kind === 'error' ? t('logs.error') : String(record.status ?? '')"
          />
          <span class="break-words">{{ summaryOf(record) }}</span>
        </SheetDescription>
      </SheetHeader>
      <div v-if="record" class="flex flex-col gap-5 px-4">
        <dl class="grid gap-x-4 gap-y-2 text-sm sm:grid-cols-[auto_1fr]">
          <template v-for="fact in facts" :key="fact.label">
            <dt class="text-muted-foreground">{{ fact.label }}</dt>
            <dd class="min-w-0 break-words font-mono text-xs leading-5">{{ fact.value }}</dd>
          </template>
          <template v-if="record.request_id">
            <dt class="text-muted-foreground">{{ t('logs.columns.requestId') }}</dt>
            <dd class="min-w-0"><CopyValue :value="record.request_id" /></dd>
          </template>
        </dl>
        <section class="flex flex-col gap-1.5">
          <div class="flex items-center justify-between gap-2">
            <h3 class="text-muted-foreground text-sm font-medium">{{ t('logs.line') }}</h3>
            <Button
              v-if="isSupported"
              variant="ghost"
              size="icon-xs"
              :aria-label="copied ? t('state.copied') : t('state.copy')"
              @click="copy(record.line)"
            >
              <Check v-if="copied" aria-hidden="true" />
              <Copy v-else aria-hidden="true" />
            </Button>
          </div>
          <pre
            class="bg-muted overflow-x-auto rounded-md p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap break-all"
            >{{ record.line }}</pre>
        </section>
        <section v-if="fields.length > 0" class="flex flex-col gap-1.5">
          <h3 class="text-muted-foreground text-sm font-medium">{{ t('logs.fields') }}</h3>
          <dl class="grid gap-x-4 gap-y-1 text-xs sm:grid-cols-[auto_1fr]">
            <template v-for="[name, value] in fields" :key="name">
              <dt class="text-muted-foreground font-mono">{{ name }}</dt>
              <dd class="min-w-0 break-words font-mono">{{ value }}</dd>
            </template>
          </dl>
        </section>
      </div>
      <SheetFooter v-if="record?.request_id" class="flex-row justify-end">
        <Button variant="outline" @click="emit('request', record.request_id)">
          <Link2 data-icon="inline-start" aria-hidden="true" />
          {{ t('logs.sameRequest') }}
        </Button>
      </SheetFooter>
    </SheetContent>
  </Sheet>
</template>
