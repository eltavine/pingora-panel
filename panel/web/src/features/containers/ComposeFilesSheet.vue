<script setup lang="ts">
import { computed } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { useClipboard } from '@vueuse/core'
import { Check, CircleAlert, Copy, FileCode, FileX } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { ComposeProjectView } from '@/api/generated'
import { projectFilesOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import { Button } from '@/components/ui/button'
import { Empty, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { engineName } from '@/lib/containers'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ engine: string; project: ComposeProjectView }>()

const { t } = useI18n()
const { copy, copied, text: copiedText, isSupported } = useClipboard({ copiedDuring: 1500 })
const files = useQuery(
  computed(() => ({
    ...projectFilesOptions({ path: { engine: props.engine, project: props.project.name } }),
    enabled: open.value,
    refetchOnWindowFocus: false,
  })),
)
const list = computed(() => files.data.value?.files ?? [])
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="flex w-full flex-col gap-0 sm:max-w-3xl">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2 break-all">
          <FileCode class="size-5 shrink-0" aria-hidden="true" />
          {{ t('containers.projects.files', { name: project.name }) }}
        </SheetTitle>
        <SheetDescription class="flex flex-wrap items-center gap-x-2">
          <span>{{ engineName(engine) }}</span>
          <span aria-hidden="true">·</span>
          <span>{{ t('containers.projects.filesDetail') }}</span>
        </SheetDescription>
      </SheetHeader>

      <div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-4 pb-4">
        <ApiFailureAlert
          v-if="files.isError.value && !files.data.value"
          :error="files.error.value"
          retryable
          @retry="files.refetch()"
        />
        <Skeleton
          v-else-if="files.isPending.value"
          class="h-64 rounded-md"
          aria-busy="true"
          :aria-label="t('state.loading')"
        />
        <Empty v-else-if="list.length === 0" class="border border-dashed">
          <EmptyHeader>
            <EmptyMedia variant="icon"><FileX aria-hidden="true" /></EmptyMedia>
            <EmptyTitle>{{ t('containers.projects.noFiles') }}</EmptyTitle>
          </EmptyHeader>
        </Empty>
        <template v-else>
          <section
            v-for="file in list"
            :key="file.path"
            class="flex flex-col gap-2"
            :aria-label="file.path"
          >
            <div class="flex items-center gap-2">
              <FileCode class="text-muted-foreground size-4 shrink-0" aria-hidden="true" />
              <h3 class="min-w-0 flex-1 font-mono text-xs font-medium break-all">
                {{ file.path }}
              </h3>
              <Button
                v-if="isSupported && typeof file.content === 'string'"
                variant="ghost"
                size="icon-xs"
                :aria-label="
                  copied && copiedText === file.content
                    ? t('state.copied')
                    : t('containers.projects.copyFile', { path: file.path })
                "
                @click="copy(file.content)"
              >
                <Check v-if="copied && copiedText === file.content" aria-hidden="true" />
                <Copy v-else aria-hidden="true" />
              </Button>
            </div>
            <pre
              v-if="typeof file.content === 'string'"
              tabindex="0"
              class="bg-muted/30 max-h-[60vh] overflow-auto rounded-md border p-3 font-mono text-xs leading-5"
              >{{ file.content }}</pre>
            <p v-else class="text-muted-foreground flex items-start gap-2 text-sm">
              <CircleAlert class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
              <span>{{
                t('containers.projects.unreadable', { reason: file.error?.message ?? '' })
              }}</span>
            </p>
          </section>
        </template>
      </div>
    </SheetContent>
  </Sheet>
</template>
