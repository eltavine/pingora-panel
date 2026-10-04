<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { CircleCheck, CircleX, ScanSearch, ShieldCheck, TriangleAlert } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { ListenerView, TlsCheck } from '@/api/generated'
import { checkTlsMutation } from '@/api/generated/@tanstack/vue-query.gen'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { STATUS_TONES } from '@/lib/certificates'
import { toApiFailure } from '@/lib/api'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ listener?: ListenerView }>()

const { t, d } = useI18n()
const run = useMutation(checkTlsMutation())
const host = ref('')
const result = ref<TlsCheck>()
const problem = ref<string>()

watch(open, () => {
  host.value = ''
  result.value = undefined
  problem.value = undefined
})

const facts = computed(() => {
  const check = result.value
  if (!check) {
    return []
  }
  return [
    { label: t('listeners.check.address'), value: check.address, mono: true },
    { label: t('listeners.check.protocol'), value: check.protocol },
    { label: t('listeners.check.cipher'), value: check.cipher_suite, mono: true },
    { label: 'ALPN', value: check.alpn ?? '-' },
    { label: t('listeners.check.handshake'), value: `${check.handshake_ms} ms` },
    {
      label: 'HSTS',
      value: check.strict_transport_security ?? t('listeners.check.noHsts'),
      mono: Boolean(check.strict_transport_security),
    },
  ]
})

function check() {
  const listener = props.listener
  if (!listener || !host.value.trim()) {
    return
  }
  problem.value = undefined
  run.mutate(
    { body: { listener: listener.id, host: host.value.trim() } },
    {
      onSuccess: (checked) => {
        result.value = checked
      },
      onError: (error) => {
        result.value = undefined
        const failure = toApiFailure(error)
        problem.value =
          failure.kind === 'problem'
            ? (failure.problem.detail ?? failure.problem.title)
            : t('listeners.check.failed')
      },
    },
  )
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-xl">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2">
          <ShieldCheck class="size-5" aria-hidden="true" />
          {{ t('listeners.check.title', { id: listener?.id ?? '' }) }}
        </SheetTitle>
        <SheetDescription>{{ t('listeners.check.description') }}</SheetDescription>
      </SheetHeader>
      <div class="flex flex-col gap-6 px-4 pb-6">
        <form class="flex gap-2" @submit.prevent="check">
          <Input
            v-model="host"
            :aria-label="t('listeners.check.host')"
            placeholder="example.com"
            class="font-mono text-xs"
            autocomplete="off"
          />
          <Button type="submit" :disabled="run.isPending.value || !host.trim()">
            <ScanSearch data-icon="inline-start" aria-hidden="true" />
            {{ t('listeners.check.run') }}
          </Button>
        </form>
        <Alert v-if="problem" variant="destructive" role="alert">
          <TriangleAlert aria-hidden="true" />
          <AlertDescription>{{ problem }}</AlertDescription>
        </Alert>
        <template v-if="result">
          <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
            <template v-for="fact in facts" :key="fact.label">
              <dt class="text-muted-foreground">{{ fact.label }}</dt>
              <dd class="min-w-0 break-words" :class="{ 'font-mono text-xs': fact.mono }">
                {{ fact.value }}
              </dd>
            </template>
          </dl>
          <section class="flex flex-col gap-2">
            <h3 class="text-sm font-medium">{{ t('listeners.check.versions') }}</h3>
            <span class="flex flex-wrap gap-2">
              <Badge
                v-for="version in result.versions"
                :key="version.version"
                :variant="version.accepted ? 'secondary' : 'outline'"
              >
                <CircleCheck v-if="version.accepted" aria-hidden="true" />
                <CircleX v-else aria-hidden="true" />
                {{ version.version }}
              </Badge>
            </span>
          </section>
          <section v-if="result.certificate" class="flex flex-col gap-2">
            <h3 class="text-sm font-medium">{{ t('listeners.check.certificate') }}</h3>
            <div class="flex flex-wrap items-center gap-2">
              <StatusIndicator
                v-if="result.certificate_status"
                :tone="STATUS_TONES[result.certificate_status]"
                :label="t(`certificates.status.${result.certificate_status}`)"
              />
              <span class="flex items-center gap-1 text-sm">
                <CircleCheck v-if="result.covers_host" class="size-4" aria-hidden="true" />
                <CircleX v-else class="text-destructive size-4" aria-hidden="true" />
                {{
                  result.covers_host
                    ? t('listeners.check.coversHost')
                    : t('listeners.check.notCovering')
                }}
              </span>
            </div>
            <span class="text-sm">{{ result.certificate.subject }}</span>
            <span class="flex flex-wrap gap-1">
              <Badge
                v-for="name in result.certificate.names"
                :key="name"
                variant="outline"
                class="font-mono"
                >{{ name }}</Badge
              >
            </span>
            <span class="text-muted-foreground text-xs"
              >{{ t('certificates.notAfter') }}:
              {{ d(new Date(result.certificate.not_after), 'datetime') }}</span
            >
          </section>
        </template>
      </div>
    </SheetContent>
  </Sheet>
</template>
