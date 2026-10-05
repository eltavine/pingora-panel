<script setup lang="ts">
import { ref, watch } from 'vue'
import { FileText, RefreshCw, Save, TriangleAlert } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import { readFile, writeFile } from '@/api/generated'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { Textarea } from '@/components/ui/textarea'
import { toApiFailure } from '@/lib/api'
import { notifyFailure, plainHeaders } from '@/lib/configuration'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ path: string; writable: boolean }>()
const emit = defineEmits<{ saved: [] }>()

const { t } = useI18n()
const content = ref('')
const tag = ref<string>()
const loading = ref(false)
const saving = ref(false)
const failure = ref<unknown>()
/** The file changed since it was read, so saving would overwrite that. */
const stale = ref(false)

async function load() {
  loading.value = true
  failure.value = undefined
  stale.value = false
  try {
    const { data, response } = await readFile({
      query: { path: props.path },
      parseAs: 'text',
      throwOnError: true,
    })
    content.value = typeof data === 'string' ? data : ''
    tag.value = response.headers.get('etag') ?? undefined
  } catch (error) {
    failure.value = error
  } finally {
    loading.value = false
  }
}

watch(
  open,
  (opened) => {
    if (opened) {
      void load()
    }
  },
  { immediate: true },
)

async function save() {
  saving.value = true
  try {
    const { response } = await writeFile({
      query: { path: props.path },
      body: new Blob([content.value], { type: 'text/plain;charset=utf-8' }),
      headers: { ...plainHeaders(), ...(tag.value ? { 'If-Match': tag.value } : {}) },
      throwOnError: true,
    })
    tag.value = response.headers.get('etag') ?? tag.value
    toast.success(t('siteFiles.saved', { path: props.path }))
    emit('saved')
    open.value = false
  } catch (error) {
    const refused = toApiFailure(error)
    if (refused.kind === 'problem' && refused.problem.status === 412) {
      stale.value = true
    } else {
      notifyFailure(error, t('common.changeFailed'))
    }
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="flex w-full flex-col gap-0 sm:max-w-3xl">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2 break-all">
          <FileText class="size-5 shrink-0" aria-hidden="true" />
          {{ path }}
        </SheetTitle>
        <SheetDescription>
          {{ writable ? t('siteFiles.editDetail') : t('siteFiles.viewDetail') }}
        </SheetDescription>
      </SheetHeader>
      <div class="flex min-h-0 flex-1 flex-col gap-3 px-4 pb-4">
        <ApiFailureAlert v-if="failure" :error="failure" retryable @retry="load" />
        <Skeleton
          v-else-if="loading"
          class="min-h-64 flex-1 rounded-md"
          aria-busy="true"
          :aria-label="t('state.loading')"
        />
        <Textarea
          v-else
          v-model="content"
          class="min-h-64 flex-1 resize-none font-mono text-xs leading-5"
          spellcheck="false"
          :readonly="!writable"
          :aria-label="t('siteFiles.content', { path })"
        />
        <p v-if="stale" class="flex items-start gap-2 text-sm" role="alert">
          <TriangleAlert class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          <span>{{ t('siteFiles.stale') }}</span>
        </p>
      </div>
      <SheetFooter v-if="writable" class="flex-row justify-end gap-2">
        <Button v-if="stale" variant="outline" @click="load">
          <RefreshCw data-icon="inline-start" aria-hidden="true" />
          {{ t('siteFiles.reload') }}
        </Button>
        <Button :disabled="saving || loading || Boolean(failure)" @click="save">
          <Save data-icon="inline-start" aria-hidden="true" />
          {{ t('common.save') }}
        </Button>
      </SheetFooter>
    </SheetContent>
  </Sheet>
</template>
