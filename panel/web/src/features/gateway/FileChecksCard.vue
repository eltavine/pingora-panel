<script setup lang="ts">
import { computed } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import {
  CircleCheck,
  FileKey,
  FolderOpen,
  FolderSearch,
  Link2Off,
  RefreshCw,
  TriangleAlert,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { fileChecksOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'

const { t, d } = useI18n()
const checks = useQuery({ ...fileChecksOptions(), retry: false })

const problems = computed(() => {
  const value = checks.data.value
  if (!value) {
    return 0
  }
  const keys = value.private_keys.filter((key) => !key.owner_only || key.error).length
  const roots = value.static_roots.reduce(
    (count, root) => count + (root.inside && !root.error ? 0 : 1) + root.escaping_links.length,
    0,
  )
  return keys + roots
})
const empty = computed(
  () =>
    checks.data.value !== undefined &&
    checks.data.value.private_keys.length === 0 &&
    checks.data.value.static_roots.length === 0,
)
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle class="flex flex-wrap items-center gap-2">
        <FolderSearch class="size-4" aria-hidden="true" />
        {{ t('gateway.files.title') }}
        <Badge v-if="checks.data.value && problems > 0" variant="destructive">
          {{ t('gateway.files.problems', { count: problems }) }}
        </Badge>
        <Badge v-else-if="checks.data.value && !empty" variant="outline">
          {{ t('gateway.files.ok') }}
        </Badge>
      </CardTitle>
      <CardDescription>
        {{ t('gateway.files.description') }}
        <span v-if="checks.data.value?.checked_at" class="whitespace-nowrap">
          ·
          {{
            t('gateway.files.checkedAt', {
              time: d(new Date(checks.data.value.checked_at), 'datetime'),
            })
          }}
        </span>
      </CardDescription>
      <CardAction>
        <Button
          variant="outline"
          size="sm"
          :disabled="checks.isFetching.value"
          @click="checks.refetch()"
        >
          <RefreshCw data-icon="inline-start" aria-hidden="true" />
          {{ t('gateway.files.refresh') }}
        </Button>
      </CardAction>
    </CardHeader>
    <CardContent class="flex flex-col gap-4">
      <ApiFailureAlert
        v-if="checks.isError.value && !checks.data.value"
        :error="checks.error.value"
        retryable
        @retry="checks.refetch()"
      />
      <Skeleton v-else-if="checks.isPending.value" class="h-16 w-full" />
      <p v-else-if="empty" class="text-muted-foreground text-sm">{{ t('gateway.files.none') }}</p>
      <template v-else-if="checks.data.value">
        <ul v-if="checks.data.value.private_keys.length > 0" class="flex flex-col gap-2">
          <li
            v-for="key in checks.data.value.private_keys"
            :key="key.file"
            class="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm"
          >
            <FileKey class="size-4 shrink-0" aria-hidden="true" />
            <span class="font-mono text-xs break-all">{{ key.file }}</span>
            <Badge v-if="key.mode" variant="outline" class="font-mono">{{ key.mode }}</Badge>
            <span v-if="key.error" class="text-destructive inline-flex items-center gap-1 text-xs">
              <TriangleAlert class="size-3.5" aria-hidden="true" />
              {{ key.error }}
            </span>
            <span
              v-else-if="key.owner_only"
              class="text-muted-foreground inline-flex items-center gap-1 text-xs"
            >
              <CircleCheck class="size-3.5" aria-hidden="true" />
              {{ t('gateway.files.ownerOnly') }}
            </span>
            <span v-else class="text-destructive inline-flex items-center gap-1 text-xs">
              <TriangleAlert class="size-3.5" aria-hidden="true" />
              {{ t('gateway.files.exposed') }}
            </span>
            <span class="text-muted-foreground font-mono text-xs">{{
              key.tls_profile_ids.join(', ')
            }}</span>
          </li>
        </ul>
        <ul v-if="checks.data.value.static_roots.length > 0" class="flex flex-col gap-2">
          <li
            v-for="root in checks.data.value.static_roots"
            :key="root.id"
            class="flex flex-col gap-1 text-sm"
          >
            <div class="flex flex-wrap items-center gap-x-3 gap-y-1">
              <FolderOpen class="size-4 shrink-0" aria-hidden="true" />
              <span class="font-mono text-xs break-all">{{ root.root }}</span>
              <span
                v-if="root.error"
                class="text-destructive inline-flex items-center gap-1 text-xs"
              >
                <TriangleAlert class="size-3.5" aria-hidden="true" />
                {{ root.error }}
              </span>
              <span
                v-else-if="root.inside"
                class="text-muted-foreground inline-flex items-center gap-1 text-xs"
              >
                <CircleCheck class="size-3.5" aria-hidden="true" />
                {{ t('gateway.files.inside') }}
              </span>
              <span v-else class="text-destructive inline-flex items-center gap-1 text-xs">
                <TriangleAlert class="size-3.5" aria-hidden="true" />
                {{ t('gateway.files.outside') }}
              </span>
              <span v-if="root.truncated" class="text-muted-foreground text-xs">
                {{ t('gateway.files.truncated', { count: root.entries_checked }) }}
              </span>
            </div>
            <ul v-if="root.escaping_links.length > 0" class="ml-7 flex flex-col gap-1">
              <li
                v-for="link in root.escaping_links"
                :key="link.path"
                class="text-destructive flex items-center gap-1.5 font-mono text-xs break-all"
                :title="t('gateway.files.linkOut')"
              >
                <Link2Off class="size-3.5 shrink-0" aria-hidden="true" />
                {{ link.path }} → {{ link.target }}
              </li>
            </ul>
          </li>
        </ul>
      </template>
    </CardContent>
  </Card>
</template>
