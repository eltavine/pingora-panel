<script setup lang="ts">
import { computed, type Component } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { Boxes, Hammer, HardDrive, Layers } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { EngineDiskUseView } from '@/api/generated'
import { diskUsageOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
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

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ engine: string }>()

const { t, d, locale } = useI18n()
const format = computed(() => formatters(locale.value))
const usage = useQuery(
  computed(() => ({
    ...diskUsageOptions({ path: { engine: props.engine } }),
    enabled: open.value,
    staleTime: 60_000,
  })),
)

const KINDS: { kind: 'images' | 'containers' | 'volumes' | 'build_cache'; icon: Component }[] = [
  { kind: 'images', icon: Layers },
  { kind: 'containers', icon: Boxes },
  { kind: 'volumes', icon: HardDrive },
  { kind: 'build_cache', icon: Hammer },
]

/** What removing what is not in use would free, as a share of the size. */
function share(use: EngineDiskUseView): number {
  return use.size_bytes > 0 ? Math.min(use.reclaimable_bytes / use.size_bytes, 1) : 0
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2">
          <HardDrive class="size-5 shrink-0" aria-hidden="true" />
          {{ t('containers.disk.title', { engine: engineName(engine) }) }}
        </SheetTitle>
        <SheetDescription>{{ t('containers.disk.description') }}</SheetDescription>
      </SheetHeader>
      <div class="flex flex-col gap-4 px-4 pb-6">
        <ApiFailureAlert
          v-if="usage.isError.value && !usage.data.value"
          :error="usage.error.value"
          retryable
          @retry="usage.refetch()"
        />
        <Skeleton
          v-else-if="usage.isPending.value"
          class="h-48 rounded-lg"
          aria-busy="true"
          :aria-label="t('containers.disk.reading')"
        />
        <template v-else-if="usage.data.value">
          <ul class="flex flex-col gap-4">
            <li
              v-for="{ kind, icon } in KINDS"
              :key="kind"
              class="flex flex-col gap-1.5 rounded-md border p-3"
            >
              <div class="flex items-center justify-between gap-2 text-sm">
                <span class="flex items-center gap-2 font-medium">
                  <component :is="icon" class="size-4" aria-hidden="true" />
                  {{ t(`containers.disk.kinds.${kind}`) }}
                </span>
                <span class="tabular-nums">{{
                  format.bytes(usage.data.value[kind].size_bytes)
                }}</span>
              </div>
              <div class="bg-muted h-1.5 overflow-hidden rounded-full" aria-hidden="true">
                <div
                  class="bg-foreground h-full rounded-full"
                  :style="{ width: `${share(usage.data.value[kind]) * 100}%` }"
                />
              </div>
              <div class="text-muted-foreground flex flex-wrap justify-between gap-x-3 text-xs">
                <span>
                  {{
                    t('containers.disk.inUse', {
                      active: format.count(usage.data.value[kind].active),
                      total: format.count(usage.data.value[kind].total),
                    })
                  }}
                </span>
                <span class="tabular-nums">
                  {{
                    t('containers.disk.reclaimable', {
                      size: format.bytes(usage.data.value[kind].reclaimable_bytes),
                    })
                  }}
                </span>
              </div>
            </li>
          </ul>
          <p v-if="usage.data.value.observed_at" class="text-muted-foreground text-xs">
            {{
              t('containers.disk.readAt', {
                time: d(new Date(usage.data.value.observed_at), 'datetime'),
              })
            }}
          </p>
        </template>
      </div>
    </SheetContent>
  </Sheet>
</template>
