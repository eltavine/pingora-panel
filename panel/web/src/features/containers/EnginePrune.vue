<script setup lang="ts">
import { computed, ref, watch, type Component } from 'vue'
import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import {
  Boxes,
  CircleCheck,
  CircleX,
  Eraser,
  Eye,
  Hammer,
  HardDrive,
  Layers,
  Network,
  Trash2,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { PruneKindName, PruneReportView } from '@/api/generated'
import { pruneMutation, prunePreviewOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Skeleton } from '@/components/ui/skeleton'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { formatters } from '@/lib/format'
import { invalidateTagged } from '@/lib/query'
import { useSession } from '@/lib/session'

const props = defineProps<{ engine: string }>()

const { t, locale } = useI18n()
const { can } = useSession()
const queryClient = useQueryClient()
const format = computed(() => formatters(locale.value))
/** How many items the preview shows before summing up the rest. */
const SHOWN = 200

const ICONS: Record<PruneKindName, Component> = {
  container: Boxes,
  image: Layers,
  volume: HardDrive,
  network: Network,
  build_cache: Hammer,
}

const taggedImages = ref(false)
const namedVolumes = ref(false)
/** Whether the preview for the current choices was asked for. */
const looking = ref(false)
const report = ref<PruneReportView>()
watch([taggedImages, namedVolumes], () => {
  looking.value = false
  report.value = undefined
})

const preview = useQuery(
  computed(() => ({
    ...prunePreviewOptions({
      path: { engine: props.engine },
      query: { tagged_images: taggedImages.value, named_volumes: namedVolumes.value },
    }),
    enabled: looking.value,
    staleTime: 0,
  })),
)
const items = computed(() => preview.data.value?.items ?? [])

function look() {
  report.value = undefined
  if (looking.value) {
    void preview.refetch()
  }
  looking.value = true
}

const prune = useMutation(pruneMutation())
const confirming = ref(false)
function confirm() {
  prune.mutate(
    {
      path: { engine: props.engine },
      body: {
        tagged_images: taggedImages.value,
        named_volumes: namedVolumes.value,
        items: items.value,
      },
      headers: plainHeaders(),
    },
    {
      onSuccess: (done) => {
        report.value = done
        const removed = done.outcomes.filter((outcome) => !outcome.error).length
        toast.success(
          t(
            'containers.prune.done',
            { count: removed, size: format.value.bytes(done.reclaimed_bytes) },
            removed,
          ),
        )
        looking.value = false
        void invalidateTagged(queryClient, ['containers'])
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
const kept = computed(() => report.value?.outcomes.filter((outcome) => outcome.error) ?? [])
</script>

<template>
  <section class="flex flex-col gap-3">
    <h3 class="flex items-center gap-2 text-sm font-medium">
      <Eraser class="size-4" aria-hidden="true" />{{ t('containers.prune.title') }}
    </h3>
    <p class="text-muted-foreground text-sm">{{ t('containers.prune.description') }}</p>
    <div class="flex flex-col gap-2">
      <label class="flex items-start gap-2 text-sm">
        <Checkbox v-model="taggedImages" class="mt-0.5" />
        <span>{{ t('containers.prune.taggedImages') }}</span>
      </label>
      <label class="flex items-start gap-2 text-sm">
        <Checkbox v-model="namedVolumes" class="mt-0.5" />
        <span>{{ t('containers.prune.namedVolumes') }}</span>
      </label>
    </div>
    <div>
      <Button variant="outline" size="sm" :disabled="preview.isFetching.value" @click="look">
        <Eye data-icon="inline-start" aria-hidden="true" />{{ t('containers.prune.look') }}
      </Button>
    </div>

    <template v-if="looking">
      <ApiFailureAlert
        v-if="preview.isError.value && !preview.data.value"
        :error="preview.error.value"
        retryable
        @retry="preview.refetch()"
      />
      <Skeleton
        v-else-if="preview.isPending.value"
        class="h-24 rounded-lg"
        aria-busy="true"
        :aria-label="t('state.loading')"
      />
      <template v-else-if="preview.data.value">
        <p v-if="!items.length" class="text-muted-foreground flex items-center gap-2 text-sm">
          <CircleCheck class="size-4" aria-hidden="true" />{{ t('containers.prune.nothing') }}
        </p>
        <template v-else>
          <ul class="flex max-h-72 flex-col overflow-y-auto rounded-md border text-sm">
            <li
              v-for="item in items.slice(0, SHOWN)"
              :key="`${item.kind}:${item.id}`"
              class="flex items-center gap-2 border-b px-3 py-1.5 last:border-b-0"
            >
              <component :is="ICONS[item.kind]" class="size-4 shrink-0" aria-hidden="true" />
              <span class="sr-only">{{ t(`containers.prune.kinds.${item.kind}`) }}</span>
              <span class="min-w-0 flex-1 font-mono text-xs break-all">{{ item.name }}</span>
              <span class="text-muted-foreground shrink-0 text-xs tabular-nums">
                {{ format.bytes(item.size_bytes) }}
              </span>
            </li>
          </ul>
          <p v-if="items.length > SHOWN" class="text-muted-foreground text-xs">
            {{ t('containers.prune.more', { count: items.length - SHOWN }, items.length - SHOWN) }}
          </p>
          <p class="text-sm">
            {{
              t(
                'containers.prune.reclaimable',
                {
                  count: items.length,
                  size: format.bytes(preview.data.value.reclaimable_bytes),
                },
                items.length,
              )
            }}
          </p>
          <div v-if="can('containers.manage')">
            <Button
              variant="destructive"
              size="sm"
              :disabled="prune.isPending.value"
              @click="confirming = true"
            >
              <Trash2 data-icon="inline-start" aria-hidden="true" />
              {{ t('containers.prune.remove', { count: items.length }, items.length) }}
            </Button>
          </div>
        </template>
      </template>
    </template>

    <div v-if="kept.length" class="flex flex-col gap-1.5">
      <p class="flex items-center gap-2 text-sm font-medium">
        <CircleX class="size-4" aria-hidden="true" />
        {{ t('containers.prune.kept', { count: kept.length }, kept.length) }}
      </p>
      <ul class="flex flex-col gap-1 text-sm">
        <li
          v-for="outcome in kept"
          :key="`${outcome.item.kind}:${outcome.item.id}`"
          class="flex flex-col"
        >
          <span class="font-mono text-xs break-all">{{ outcome.item.name }}</span>
          <span class="text-muted-foreground text-xs">{{ outcome.error?.message }}</span>
        </li>
      </ul>
    </div>

    <ConfirmDialog
      v-model:open="confirming"
      :icon="Trash2"
      :title="t('containers.prune.confirmTitle', { count: items.length }, items.length)"
      :description="
        t('containers.prune.confirmDetail', {
          size: format.bytes(preview.data.value?.reclaimable_bytes ?? 0),
        })
      "
      :confirm-label="t('containers.actions.remove')"
      destructive
      :busy="prune.isPending.value"
      @confirm="confirm"
    />
  </section>
</template>
