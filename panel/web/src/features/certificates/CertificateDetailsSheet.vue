<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useQueryClient } from '@tanstack/vue-query'
import { CircleCheck, CircleX, Download, FileBadge, ScanSearch } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { CertificateCoverage, CertificateView } from '@/api/generated'
import { certificateCoverageOptions } from '@/api/generated/@tanstack/vue-query.gen'
import CopyValue from '@/components/CopyValue.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
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
import { notifyFailure } from '@/lib/configuration'
import { downloadFile } from '@/lib/download'
import { KEY_ALGORITHMS, fingerprint, parseNames } from './presentation'
import { STATUS_TONES } from '@/lib/certificates'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ certificate?: CertificateView }>()

const { t, d } = useI18n()
const client = useQueryClient()

const hosts = ref('')
const coverage = ref<CertificateCoverage>()
const checking = ref(false)
watch(open, () => {
  hosts.value = ''
  coverage.value = undefined
})

const fields = computed(() => {
  const certificate = props.certificate
  if (!certificate) {
    return []
  }
  return [
    { label: t('certificates.subject'), value: certificate.subject },
    { label: t('certificates.columns.issuer'), value: certificate.issuer },
    { label: t('certificates.serial'), value: certificate.serial, mono: true },
    { label: t('certificates.notBefore'), value: d(new Date(certificate.not_before), 'datetime') },
    { label: t('certificates.notAfter'), value: d(new Date(certificate.not_after), 'datetime') },
    {
      label: t('certificates.keyAlgorithm'),
      value: `${KEY_ALGORITHMS[certificate.key_algorithm] ?? certificate.key_algorithm} · ${t('certificates.keyBits', { bits: certificate.key_bits })}`,
    },
    { label: t('certificates.chainLength'), value: String(certificate.chain_length) },
    { label: t('certificates.version'), value: String(certificate.version) },
  ]
})

async function check() {
  const certificate = props.certificate
  const names = parseNames(hosts.value)
  if (!certificate || names.length === 0) {
    return
  }
  checking.value = true
  try {
    coverage.value = await client.fetchQuery(
      certificateCoverageOptions({
        path: { id: certificate.id },
        query: { hosts: names.join(',') },
      }),
    )
  } catch (error) {
    notifyFailure(error, t('certificates.checkFailed'))
  } finally {
    checking.value = false
  }
}

function download() {
  const certificate = props.certificate
  if (!certificate) {
    return
  }
  downloadFile(`${certificate.id}.pem`, certificate.chain, 'application/x-pem-file')
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-xl">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2">
          <FileBadge class="size-5" aria-hidden="true" />
          <span class="font-mono">{{ certificate?.id }}</span>
        </SheetTitle>
        <SheetDescription>{{ t('certificates.details') }}</SheetDescription>
      </SheetHeader>
      <div v-if="certificate" class="flex flex-col gap-6 px-4 pb-6">
        <div class="flex flex-wrap items-center gap-2">
          <StatusIndicator
            :tone="STATUS_TONES[certificate.status]"
            :label="t(`certificates.status.${certificate.status}`)"
          />
          <Badge variant="secondary">{{ t(`certificates.source.${certificate.source}`) }}</Badge>
        </div>
        <section class="flex flex-col gap-2">
          <h3 class="text-sm font-medium">{{ t('certificates.columns.names') }}</h3>
          <span class="flex flex-wrap gap-1">
            <Badge
              v-for="name in certificate.names"
              :key="name"
              variant="outline"
              class="font-mono"
              >{{ name }}</Badge
            >
          </span>
        </section>
        <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
          <template v-for="field in fields" :key="field.label">
            <dt class="text-muted-foreground">{{ field.label }}</dt>
            <dd class="min-w-0 break-words" :class="{ 'font-mono text-xs': field.mono }">
              {{ field.value }}
            </dd>
          </template>
        </dl>
        <section class="flex flex-col gap-2">
          <h3 class="text-sm font-medium">{{ t('certificates.fingerprint') }}</h3>
          <CopyValue :value="fingerprint(certificate.fingerprint)" />
          <h3 class="text-sm font-medium">{{ t('certificates.publicKeyFingerprint') }}</h3>
          <CopyValue :value="fingerprint(certificate.public_key_fingerprint)" />
        </section>
        <Button variant="outline" size="sm" class="self-start" @click="download">
          <Download data-icon="inline-start" aria-hidden="true" />
          {{ t('certificates.downloadChain') }}
        </Button>
        <section class="flex flex-col gap-2">
          <h3 class="text-sm font-medium">{{ t('certificates.coverage') }}</h3>
          <form class="flex gap-2" @submit.prevent="check">
            <Input
              v-model="hosts"
              :aria-label="t('certificates.coverage')"
              :placeholder="certificate.names[0]"
              class="font-mono text-xs"
            />
            <Button type="submit" variant="secondary" :disabled="checking || !hosts.trim()">
              <ScanSearch data-icon="inline-start" aria-hidden="true" />
              {{ t('certificates.check') }}
            </Button>
          </form>
          <p class="text-muted-foreground text-xs">{{ t('certificates.coverageHint') }}</p>
          <ul v-if="coverage" class="flex flex-col gap-1 text-sm">
            <li v-for="host in coverage.hosts" :key="host.host" class="flex items-center gap-2">
              <CircleCheck v-if="host.covered" class="size-4" aria-hidden="true" />
              <CircleX v-else class="text-destructive size-4" aria-hidden="true" />
              <span class="font-mono text-xs">{{ host.host }}</span>
              <span class="text-muted-foreground text-xs">{{
                host.covered ? t('certificates.covered') : t('certificates.notCovered')
              }}</span>
            </li>
          </ul>
        </section>
      </div>
    </SheetContent>
  </Sheet>
</template>
