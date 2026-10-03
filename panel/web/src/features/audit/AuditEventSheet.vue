<script setup lang="ts">
import { computed } from 'vue'
import { Link2, ScrollText } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { AuditEvent } from '@/api/generated'
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
import { isKnownType, toneOf, typeKey } from './presentation'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ event?: AuditEvent }>()
const emit = defineEmits<{ correlate: [correlation: string] }>()

const { t, d } = useI18n()

const label = computed(() => {
  const type = props.event?.event_type ?? ''
  return isKnownType(type) ? t(typeKey(type)) : type
})

const facts = computed(() => {
  const event = props.event
  if (!event) {
    return []
  }
  const time = (value?: string | null) => (value ? d(new Date(value), 'datetime') : '')
  return [
    { label: t('audit.columns.type'), value: `${event.event_type} v${event.event_version}` },
    { label: t('audit.columns.actor'), value: `${event.actor_id} (${event.actor_type})` },
    { label: t('audit.columns.subject'), value: event.subject },
    { label: t('audit.occurred'), value: time(event.occurred_at) },
    { label: t('audit.recorded'), value: time(event.recorded_at) },
    { label: t('audit.source'), value: event.source },
    { label: t('audit.causation'), value: event.causation_id },
    { label: t('audit.idempotencyKey'), value: event.idempotency_key ?? '' },
  ].filter((fact) => fact.value)
})
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-2xl">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2">
          <ScrollText class="size-5" aria-hidden="true" />
          {{ t('audit.detail', { sequence: event?.sequence ?? '' }) }}
        </SheetTitle>
        <SheetDescription>
          <StatusIndicator v-if="event" :tone="toneOf(event.event_type)" :label="label" />
        </SheetDescription>
      </SheetHeader>
      <div v-if="event" class="flex flex-col gap-5 px-4">
        <dl class="grid gap-x-4 gap-y-2 text-sm sm:grid-cols-[auto_1fr]">
          <template v-for="fact in facts" :key="fact.label">
            <dt class="text-muted-foreground">{{ fact.label }}</dt>
            <dd class="min-w-0 break-words font-mono text-xs leading-5">{{ fact.value }}</dd>
          </template>
          <dt class="text-muted-foreground">{{ t('audit.columns.correlation') }}</dt>
          <dd class="min-w-0"><CopyValue :value="event.correlation_id" /></dd>
          <dt class="text-muted-foreground">{{ t('audit.hash') }}</dt>
          <dd class="min-w-0"><CopyValue :value="event.hash" /></dd>
          <dt class="text-muted-foreground">{{ t('audit.previousHash') }}</dt>
          <dd class="min-w-0">
            <CopyValue v-if="event.previous_hash" :value="event.previous_hash" />
            <span v-else class="text-muted-foreground text-xs">{{ t('audit.genesis') }}</span>
          </dd>
        </dl>
        <section class="flex flex-col gap-1.5">
          <h3 class="text-muted-foreground text-sm font-medium">{{ t('audit.data') }}</h3>
          <pre class="bg-muted overflow-x-auto rounded-md p-3 font-mono text-xs leading-relaxed">{{
            JSON.stringify(event.data, null, 2)
          }}</pre>
        </section>
      </div>
      <SheetFooter v-if="event" class="flex-row justify-end">
        <Button variant="outline" @click="emit('correlate', event.correlation_id)">
          <Link2 data-icon="inline-start" aria-hidden="true" />
          {{ t('audit.sameRequest') }}
        </Button>
      </SheetFooter>
    </SheetContent>
  </Sheet>
</template>
