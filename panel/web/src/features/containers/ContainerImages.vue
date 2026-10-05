<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import { refDebounced } from '@vueuse/core'
import { Download, Layers, Search, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { ImageView } from '@/api/generated'
import {
  listImagesOptions,
  listImagesQueryKey,
  removeImageMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Empty, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { formatters } from '@/lib/format'
import { useSession } from '@/lib/session'
import ImageDetailSheet from './ImageDetailSheet.vue'
import ImagePullSheet from './ImagePullSheet.vue'
import { imageName, REFRESH_INTERVAL_MS, shortId } from './presentation'

const props = defineProps<{ engine: string }>()

const { t, d, locale } = useI18n()
const { can } = useSession()
const queryClient = useQueryClient()
const format = computed(() => formatters(locale.value))

const search = ref('')
const searched = refDebounced(search, 300)
const images = useQuery(
  computed(() => ({
    ...listImagesOptions({
      path: { engine: props.engine },
      query: { search: searched.value.trim() || undefined },
    }),
    refetchInterval: REFRESH_INTERVAL_MS,
  })),
)
const rows = computed(() => images.data.value?.images ?? [])

const pullOpen = ref(false)
function pulled() {
  void queryClient.invalidateQueries({
    queryKey: listImagesQueryKey({ path: { engine: props.engine } }),
  })
}

const inspecting = ref<ImageView>()
const detailOpen = ref(false)
function inspect(image: ImageView) {
  inspecting.value = image
  detailOpen.value = true
}

const remove = useMutation(removeImageMutation())
const removing = ref<ImageView>()
const removeOpen = computed({
  get: () => removing.value !== undefined,
  set: (open: boolean) => {
    if (!open) {
      removing.value = undefined
    }
  },
})
const force = ref(false)

function openRemove(image: ImageView) {
  force.value = false
  removing.value = image
}

function confirmRemove() {
  const image = removing.value
  if (!image) {
    return
  }
  remove.mutate(
    {
      path: { engine: props.engine, image: image.id },
      query: { force: force.value },
      headers: plainHeaders(),
    },
    {
      onSuccess: () => {
        toast.success(t('containers.images.removed', { image: imageName(image) }))
        void queryClient.invalidateQueries({
          queryKey: listImagesQueryKey({ path: { engine: props.engine } }),
        })
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <div class="flex flex-col gap-4">
    <div class="flex flex-wrap items-center gap-2">
      <div class="relative min-w-56 flex-1">
        <Search
          class="text-muted-foreground absolute top-1/2 left-2.5 size-4 -translate-y-1/2"
          aria-hidden="true"
        />
        <Input
          v-model="search"
          type="search"
          class="pl-8"
          :placeholder="t('containers.images.searchPlaceholder')"
          :aria-label="t('containers.images.search')"
        />
      </div>
      <Button v-if="can('containers.manage')" variant="outline" @click="pullOpen = true">
        <Download data-icon="inline-start" aria-hidden="true" />
        {{ t('containers.pull.open') }}
      </Button>
    </div>
    <ApiFailureAlert
      v-if="images.isError.value && !images.data.value"
      :error="images.error.value"
      retryable
      @retry="images.refetch()"
    />
    <Skeleton
      v-else-if="images.isPending.value"
      class="h-24 rounded-lg"
      aria-busy="true"
      :aria-label="t('state.loading')"
    />
    <div v-else-if="rows.length" class="overflow-x-auto">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{{ t('containers.images.tags') }}</TableHead>
            <TableHead class="hidden sm:table-cell">{{ t('containers.images.id') }}</TableHead>
            <TableHead>{{ t('containers.images.size') }}</TableHead>
            <TableHead>{{ t('containers.images.containers') }}</TableHead>
            <TableHead class="hidden xl:table-cell">{{ t('containers.images.created') }}</TableHead>
            <TableHead v-if="can('containers.manage')">
              <span class="sr-only">{{ t('common.actions') }}</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="image in rows" :key="image.id">
            <TableCell>
              <div class="flex flex-col items-start gap-1">
                <Button
                  variant="link"
                  class="h-auto justify-start p-0 text-left font-mono text-xs break-all whitespace-normal"
                  @click="inspect(image)"
                >
                  {{ image.tags[0] ?? shortId(image.id) }}
                </Button>
                <Badge v-if="!image.tags.length" variant="outline">
                  {{ t('containers.images.untagged') }}
                </Badge>
                <span
                  v-for="tag in image.tags.slice(1)"
                  :key="tag"
                  class="text-muted-foreground font-mono text-xs break-all"
                >
                  {{ tag }}
                </span>
              </div>
            </TableCell>
            <TableCell class="text-muted-foreground hidden font-mono text-xs sm:table-cell">
              {{ shortId(image.id) }}
            </TableCell>
            <TableCell class="text-sm whitespace-nowrap tabular-nums">
              {{ format.bytes(image.size_bytes) }}
            </TableCell>
            <TableCell class="text-sm whitespace-nowrap">
              <span :class="{ 'text-muted-foreground': image.containers === 0 }">
                {{ t('containers.images.used', { n: image.containers }, image.containers) }}
              </span>
            </TableCell>
            <TableCell class="text-muted-foreground hidden text-sm whitespace-nowrap xl:table-cell">
              {{ image.created ? d(new Date(image.created), 'datetime') : '—' }}
            </TableCell>
            <TableCell v-if="can('containers.manage')" class="text-right">
              <Button
                variant="ghost"
                size="icon-sm"
                :disabled="remove.isPending.value"
                :aria-label="t('containers.images.removeOne', { image: imageName(image) })"
                @click="openRemove(image)"
              >
                <Trash2 aria-hidden="true" />
              </Button>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
    <Empty v-else class="border border-dashed">
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <Search v-if="searched" aria-hidden="true" />
          <Layers v-else aria-hidden="true" />
        </EmptyMedia>
        <EmptyTitle>
          {{ searched ? t('containers.images.noMatch') : t('containers.images.empty') }}
        </EmptyTitle>
      </EmptyHeader>
    </Empty>

    <ImagePullSheet
      v-if="can('containers.manage')"
      v-model:open="pullOpen"
      :engine="engine"
      @pulled="pulled"
    />
    <ImageDetailSheet
      v-if="inspecting"
      v-model:open="detailOpen"
      :engine="engine"
      :image="inspecting"
    />
    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="t('containers.images.removeTitle', { image: removing ? imageName(removing) : '' })"
      :description="t('containers.images.removeDetail')"
      :confirm-label="t('containers.actions.remove')"
      destructive
      :busy="remove.isPending.value"
      @confirm="confirmRemove"
    >
      <label class="flex items-start gap-2 text-sm">
        <Checkbox v-model="force" class="mt-0.5" />
        <span>{{ t('containers.images.force') }}</span>
      </label>
    </ConfirmDialog>
  </div>
</template>
