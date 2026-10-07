<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { HardDrive, Save } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import {
  cacheSettingsOptions,
  putCacheSettingsMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Skeleton } from '@/components/ui/skeleton'
import { notifyFailure, plainHeaders, useRefreshConfiguration } from '@/lib/configuration'
import { printSize } from '@/lib/forms'
import { storeBytes } from './forms'

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const settings = useQuery(cacheSettingsOptions())
const put = useMutation(putCacheSettingsMutation())

const size = ref('')
watch(
  () => settings.data.value,
  (value) => {
    if (value) {
      size.value = printSize(value.max_bytes)
    }
  },
  { immediate: true },
)
const bytes = computed(() => storeBytes(size.value))
const unchanged = computed(() => bytes.value === (settings.data.value?.max_bytes ?? null))

function save() {
  put.mutate(
    { body: { max_bytes: bytes.value }, headers: plainHeaders() },
    {
      onSuccess: () => {
        toast.success(t('cache.store.saved'))
        void refresh()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle class="flex items-center gap-2">
        <HardDrive class="size-4" aria-hidden="true" />
        {{ t('cache.store.title') }}
      </CardTitle>
      <CardDescription>{{ t('cache.store.hint') }}</CardDescription>
    </CardHeader>
    <CardContent>
      <ApiFailureAlert
        v-if="settings.isError.value && !settings.data.value"
        :error="settings.error.value"
        retryable
        @retry="settings.refetch()"
      />
      <Skeleton v-else-if="!settings.data.value" class="h-16 w-full" />
      <form v-else class="flex flex-col gap-1.5" @submit.prevent="save">
        <Label for="cache-store-size">{{ t('cache.store.size') }}</Label>
        <div class="flex max-w-xs gap-2">
          <Input
            id="cache-store-size"
            v-model="size"
            placeholder="256m"
            class="font-mono text-xs"
            autocomplete="off"
            :aria-invalid="Number.isNaN(bytes)"
            aria-describedby="cache-store-size-hint"
          />
          <Button
            type="submit"
            variant="outline"
            :disabled="put.isPending.value || Number.isNaN(bytes) || unchanged"
          >
            <Save data-icon="inline-start" aria-hidden="true" />
            {{ t('common.save') }}
          </Button>
        </div>
        <p
          id="cache-store-size-hint"
          class="text-xs"
          :class="Number.isNaN(bytes) ? 'text-destructive' : 'text-muted-foreground'"
        >
          {{ Number.isNaN(bytes) ? t('cache.store.invalid') : t('cache.store.sizeHint') }}
        </p>
      </form>
    </CardContent>
  </Card>
</template>
