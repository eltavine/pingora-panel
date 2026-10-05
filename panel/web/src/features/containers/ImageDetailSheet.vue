<script setup lang="ts">
import { computed } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { Layers, Tags } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { ImageView } from '@/api/generated'
import { inspectImageOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import { Badge } from '@/components/ui/badge'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { engineName } from '@/lib/containers'
import { formatters } from '@/lib/format'
import { imageName, sortedLabels } from './presentation'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ engine: string; image: ImageView }>()

const { t, d, locale } = useI18n()
const format = computed(() => formatters(locale.value))
const detail = useQuery(
  computed(() => ({
    ...inspectImageOptions({ path: { engine: props.engine, image: props.image.id } }),
    enabled: open.value,
  })),
)
const view = computed(() => detail.data.value)

const facts = computed(() => {
  const value = view.value
  if (!value) {
    return []
  }
  const platform = [value.os, value.architecture, value.variant].filter(Boolean).join('/')
  return [
    { label: t('containers.images.id'), value: value.image.id, mono: true },
    { label: t('containers.images.digests'), value: value.image.digests.join('\n'), mono: true },
    {
      label: t('containers.images.created'),
      value: value.image.created ? d(new Date(value.image.created), 'datetime') : '',
      mono: false,
    },
    {
      label: t('containers.images.size'),
      value: format.value.bytes(value.image.size_bytes),
      mono: false,
    },
    {
      label: t('containers.images.containers'),
      value: t('containers.images.used', { n: value.image.containers }, value.image.containers),
      mono: false,
    },
    { label: t('containers.images.platform'), value: platform, mono: true },
    { label: t('containers.images.author'), value: value.author ?? '', mono: false },
    { label: t('containers.detail.user'), value: value.user ?? '', mono: true },
    {
      label: t('containers.detail.workingDirectory'),
      value: value.working_directory ?? '',
      mono: true,
    },
    {
      label: t('containers.images.exposedPorts'),
      value: value.exposed_ports.join(', '),
      mono: true,
    },
    { label: t('containers.images.volumes'), value: value.volumes.join('\n'), mono: true },
    { label: t('containers.images.stopSignal'), value: value.stop_signal ?? '', mono: true },
    { label: t('containers.images.layers'), value: String(value.layers), mono: false },
  ].filter((fact) => fact.value)
})
const labels = computed(() => sortedLabels(view.value?.image.labels ?? {}))
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-xl">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2 break-all">
          <Layers class="size-5 shrink-0" aria-hidden="true" />{{ imageName(image) }}
        </SheetTitle>
        <SheetDescription class="flex flex-wrap items-center gap-x-2">
          <span>{{ engineName(engine) }}</span>
          <template v-for="tag in image.tags.slice(1)" :key="tag">
            <span aria-hidden="true">·</span>
            <span class="font-mono text-xs break-all">{{ tag }}</span>
          </template>
        </SheetDescription>
      </SheetHeader>
      <div class="flex flex-col gap-6 px-4 pb-6">
        <ApiFailureAlert
          v-if="detail.isError.value && !view"
          :error="detail.error.value"
          retryable
          @retry="detail.refetch()"
        />
        <Skeleton
          v-else-if="detail.isPending.value"
          class="h-40 rounded-lg"
          aria-busy="true"
          :aria-label="t('state.loading')"
        />
        <template v-else-if="view">
          <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
            <template v-for="fact in facts" :key="fact.label">
              <dt class="text-muted-foreground">{{ fact.label }}</dt>
              <dd
                class="min-w-0 break-words whitespace-pre-line"
                :class="{ 'font-mono text-xs': fact.mono }"
              >
                {{ fact.value }}
              </dd>
            </template>
          </dl>
          <section class="flex flex-col gap-2">
            <h3 class="flex items-center gap-2 text-sm font-medium">
              <Tags class="size-4" aria-hidden="true" />{{ t('containers.detail.labels') }}
            </h3>
            <dl v-if="labels.length" class="flex flex-col gap-1.5">
              <div v-for="label in labels" :key="label.name" class="flex min-w-0 flex-col">
                <dt class="text-muted-foreground font-mono text-xs break-all">{{ label.name }}</dt>
                <dd class="font-mono text-xs break-all">{{ label.value || '—' }}</dd>
              </div>
            </dl>
            <p v-else class="text-muted-foreground text-sm">
              {{ t('containers.detail.noLabels') }}
            </p>
          </section>
          <p v-if="!view.image.tags.length" class="text-muted-foreground text-sm">
            <Badge variant="outline">{{ t('containers.images.untagged') }}</Badge>
          </p>
        </template>
      </div>
    </SheetContent>
  </Sheet>
</template>
