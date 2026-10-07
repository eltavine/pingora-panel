<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Activity, Clock, DatabaseZap, Eraser, HardDrive, Layers, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { CachePurgeRequest } from '@/api/generated'
import {
  cacheStatsOptions,
  listSitesOptions,
  purgeCacheMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import FormField from '@/components/FormField.vue'
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
import { Progress } from '@/components/ui/progress'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { Textarea } from '@/components/ui/textarea'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { formatters } from '@/lib/format'
import { lines } from '@/lib/forms'
import { lookups } from './forms'

const REFRESH_INTERVAL_MS = 10_000

const { t, d, locale } = useI18n()
const format = computed(() => formatters(locale.value))
const stats = useQuery({
  ...cacheStatsOptions(),
  refetchInterval: REFRESH_INTERVAL_MS,
  retry: false,
})
const sites = useQuery(listSitesOptions({ query: { limit: 500 } }))
const purge = useMutation(purgeCacheMutation())

const siteNames = computed(
  () => new Map((sites.data.value?.items ?? []).map((site) => [site.id, site.name])),
)
const usage = computed(() => {
  const value = stats.data.value
  return value && value.max_bytes > 0 ? Math.min(100, (value.bytes / value.max_bytes) * 100) : 0
})
const facts = computed(() => {
  const value = stats.data.value
  if (!value) {
    return []
  }
  return [
    {
      icon: HardDrive,
      label: t('cache.stats.size'),
      value: t('cache.stats.sizeOf', {
        used: format.value.bytes(value.bytes),
        max: format.value.bytes(value.max_bytes),
      }),
    },
    { icon: Layers, label: t('cache.stats.entries'), value: format.value.count(value.entries) },
    {
      icon: Clock,
      label: t('cache.stats.since'),
      value: value.since ? d(new Date(value.since), 'datetime') : t('state.none'),
    },
  ]
})

const urls = ref('')
const confirmAll = ref(false)
const purgingSite = ref<string | null>(null)
const siteOpen = computed({
  get: () => purgingSite.value !== null,
  set: (open) => {
    if (!open) {
      purgingSite.value = null
    }
  },
})

function run(body: CachePurgeRequest, done: () => void) {
  purge.mutate(
    { body, headers: plainHeaders() },
    {
      onSuccess: (result) => {
        toast.success(
          body.urls?.length
            ? t('cache.purge.keys', { count: result.keys }, result.keys)
            : t('cache.purge.done'),
        )
        done()
        void stats.refetch()
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

function purgeUrls() {
  run({ urls: lines(urls.value) }, () => (urls.value = ''))
}
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle class="flex items-center gap-2">
        <Activity class="size-4" aria-hidden="true" />
        {{ t('cache.stats.title') }}
      </CardTitle>
      <CardDescription>{{ t('cache.stats.hint') }}</CardDescription>
      <CardAction>
        <Button
          variant="destructive"
          size="sm"
          :disabled="!stats.data.value"
          @click="confirmAll = true"
        >
          <Trash2 data-icon="inline-start" aria-hidden="true" />
          {{ t('cache.purge.all') }}
        </Button>
      </CardAction>
    </CardHeader>
    <CardContent class="flex flex-col gap-6">
      <ApiFailureAlert
        v-if="stats.isError.value && !stats.data.value"
        :error="stats.error.value"
        retryable
        @retry="stats.refetch()"
      />
      <Skeleton v-else-if="!stats.data.value" class="h-32 w-full" />
      <template v-else>
        <div class="flex flex-col gap-2">
          <Progress :model-value="usage" :aria-label="t('cache.stats.size')" />
          <dl class="grid gap-4 sm:grid-cols-3">
            <div v-for="fact in facts" :key="fact.label" class="flex min-w-0 flex-col gap-1">
              <dt class="text-muted-foreground flex items-center gap-1.5 text-xs">
                <component :is="fact.icon" class="size-3.5" aria-hidden="true" />
                {{ fact.label }}
              </dt>
              <dd class="truncate font-mono text-sm">{{ fact.value }}</dd>
            </div>
          </dl>
        </div>

        <p
          v-if="stats.data.value.sites.length === 0"
          class="text-muted-foreground flex items-center gap-2 text-sm"
        >
          <DatabaseZap class="size-4" aria-hidden="true" />
          {{ t('cache.stats.noLookups') }}
        </p>
        <div v-else class="rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{{ t('cache.stats.site') }}</TableHead>
                <TableHead>{{ t('cache.stats.hitRatio') }}</TableHead>
                <TableHead class="text-right">{{ t('cache.stats.served') }}</TableHead>
                <TableHead class="text-right">{{ t('cache.stats.fetched') }}</TableHead>
                <TableHead class="text-right">{{ t('cache.stats.bypassed') }}</TableHead>
                <TableHead class="w-12"
                  ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
                >
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="site in stats.data.value.sites" :key="site.site_id">
                <TableCell>
                  <RouterLink :to="`/sites/${site.site_id}`" class="text-sm hover:underline">
                    {{ siteNames.get(site.site_id) ?? site.site_id }}
                  </RouterLink>
                </TableCell>
                <TableCell>
                  <Badge variant="outline" class="font-mono">
                    {{
                      site.hit_ratio === null || site.hit_ratio === undefined
                        ? '—'
                        : format.percent(site.hit_ratio)
                    }}
                  </Badge>
                </TableCell>
                <TableCell class="text-right font-mono text-xs">{{
                  format.count(lookups(site).served)
                }}</TableCell>
                <TableCell class="text-right font-mono text-xs">{{
                  format.count(site.misses + site.expired + site.uncacheable)
                }}</TableCell>
                <TableCell class="text-right font-mono text-xs">{{
                  format.count(site.bypasses)
                }}</TableCell>
                <TableCell>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    :aria-label="t('cache.purge.site')"
                    :title="t('cache.purge.site')"
                    @click="purgingSite = site.site_id"
                  >
                    <Eraser aria-hidden="true" />
                  </Button>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>

        <form class="flex flex-col gap-2" @submit.prevent="purgeUrls">
          <FormField
            id="cache-purge-urls"
            :label="t('cache.purge.urls')"
            :hint="t('cache.purge.urlsHint')"
          >
            <Textarea
              id="cache-purge-urls"
              v-model="urls"
              rows="2"
              class="font-mono text-xs"
              placeholder="https://shop.example/&#10;https://shop.example/products?page=2"
            />
          </FormField>
          <div>
            <Button
              type="submit"
              variant="outline"
              size="sm"
              :disabled="purge.isPending.value || lines(urls).length === 0"
            >
              <Eraser data-icon="inline-start" aria-hidden="true" />
              {{ t('cache.purge.urlsAction') }}
            </Button>
          </div>
        </form>
      </template>
    </CardContent>
  </Card>

  <ConfirmDialog
    v-model:open="confirmAll"
    :icon="Trash2"
    :title="t('cache.purge.allTitle')"
    :description="t('cache.purge.allDetail')"
    :confirm-label="t('cache.purge.all')"
    destructive
    :busy="purge.isPending.value"
    @confirm="run({ all: true }, () => (confirmAll = false))"
  />
  <ConfirmDialog
    v-model:open="siteOpen"
    :icon="Eraser"
    :title="
      t('cache.purge.siteTitle', {
        site: purgingSite ? (siteNames.get(purgingSite) ?? purgingSite) : '',
      })
    "
    :description="t('cache.purge.siteDetail')"
    :confirm-label="t('cache.purge.site')"
    :busy="purge.isPending.value"
    @confirm="run({ site_ids: [purgingSite!] }, () => (purgingSite = null))"
  />
</template>
