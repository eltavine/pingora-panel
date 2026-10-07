<script setup lang="ts">
import { computed, ref } from 'vue'
import { useQuery, useQueryClient } from '@tanstack/vue-query'
import {
  Cable,
  ChevronRight,
  Clock,
  FolderSearch,
  Puzzle,
  RefreshCw,
  ShieldCheck,
  ShieldOff,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import { discoverPlugins } from '@/api/generated'
import { listPluginsOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
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
import { invalidateTagged } from '@/lib/query'
import { useSession } from '@/lib/session'
import { portIcon, portsOf, stateTone, targetVersion } from './presentation'
import SecretsCard from './SecretsCard.vue'
import TrustedKeysCard from './TrustedKeysCard.vue'

const { t, d } = useI18n()
const client = useQueryClient()
const { can } = useSession()
const manages = computed(() => can('plugins.manage'))

const listing = useQuery(listPluginsOptions())
const plugins = computed(() => listing.data.value?.plugins ?? [])

const discovering = ref(false)
async function discover() {
  discovering.value = true
  try {
    await discoverPlugins({ headers: plainHeaders(), throwOnError: true })
    toast.success(t('plugins.discovered'))
    void invalidateTagged(client, ['plugins'])
  } catch (error) {
    notifyFailure(error, t('plugins.discoverFailed'))
  } finally {
    discovering.value = false
  }
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader :icon="Puzzle" :title="t('plugins.title')" :description="t('plugins.description')">
      <template #actions>
        <Button
          variant="outline"
          size="sm"
          :disabled="listing.isFetching.value"
          @click="listing.refetch()"
        >
          <RefreshCw data-icon="inline-start" aria-hidden="true" />
          {{ t('plugins.refresh') }}
        </Button>
        <Button v-if="manages" size="sm" :disabled="discovering" @click="discover">
          <FolderSearch data-icon="inline-start" aria-hidden="true" />
          {{ t('plugins.discover') }}
        </Button>
      </template>
    </PageHeader>

    <div v-if="listing.data.value" class="flex flex-wrap items-center gap-2 text-sm">
      <Badge variant="outline" class="gap-1">
        <Cable class="size-3" aria-hidden="true" />
        {{
          t('plugins.host.protocol', { versions: listing.data.value.protocol_versions.join(', ') })
        }}
      </Badge>
      <Badge v-for="port in listing.data.value.ports" :key="port" variant="secondary" class="gap-1">
        <component :is="portIcon(port)" class="size-3" aria-hidden="true" />
        {{ t(`plugins.portNames.${port}`) }}
      </Badge>
      <Badge variant="outline" class="gap-1">
        <ShieldCheck v-if="listing.data.value.limits_enforced" class="size-3" aria-hidden="true" />
        <ShieldOff v-else class="size-3" aria-hidden="true" />
        {{
          listing.data.value.limits_enforced
            ? t('plugins.host.enforced')
            : t('plugins.host.notEnforced')
        }}
      </Badge>
      <span class="text-muted-foreground inline-flex items-center gap-1 text-xs">
        <Clock class="size-3" aria-hidden="true" />
        {{
          listing.data.value.discovered_at
            ? t('plugins.host.discoveredAt', {
                time: d(new Date(listing.data.value.discovered_at), 'datetime'),
              })
            : t('plugins.host.never')
        }}
      </span>
    </div>

    <Card>
      <CardContent class="flex flex-col gap-4">
        <ApiFailureAlert
          v-if="listing.isError.value && !listing.data.value"
          :error="listing.error.value"
          retryable
          @retry="listing.refetch()"
        />
        <Skeleton
          v-else-if="listing.isPending.value"
          class="h-24 rounded-lg"
          aria-busy="true"
          :aria-label="t('state.loading')"
        />
        <div v-else-if="plugins.length" class="overflow-x-auto">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{{ t('plugins.name') }}</TableHead>
                <TableHead>{{ t('plugins.state') }}</TableHead>
                <TableHead class="hidden sm:table-cell">{{ t('plugins.version') }}</TableHead>
                <TableHead class="hidden md:table-cell">{{ t('plugins.ports') }}</TableHead>
                <TableHead class="hidden lg:table-cell">{{ t('plugins.grants') }}</TableHead>
                <TableHead>
                  <span class="sr-only">{{ t('common.actions') }}</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="plugin in plugins" :key="plugin.name">
                <TableCell class="align-top">
                  <RouterLink
                    :to="`/plugins/${plugin.name}`"
                    class="flex flex-col gap-0.5 hover:underline"
                  >
                    <span class="font-medium">{{ plugin.name }}</span>
                    <span class="text-muted-foreground max-w-72 truncate text-xs">
                      {{ targetVersion(plugin)?.description }}
                    </span>
                  </RouterLink>
                </TableCell>
                <TableCell class="align-top">
                  <StatusIndicator
                    :tone="stateTone(plugin.state)"
                    :label="t(`plugins.states.${plugin.state}`)"
                  />
                </TableCell>
                <TableCell class="hidden align-top font-mono text-sm sm:table-cell">
                  {{ plugin.active_version ?? targetVersion(plugin)?.version ?? t('plugins.none') }}
                </TableCell>
                <TableCell class="hidden align-top md:table-cell">
                  <div class="flex flex-wrap gap-1">
                    <Badge
                      v-for="port in portsOf(plugin)"
                      :key="port"
                      variant="secondary"
                      class="gap-1"
                    >
                      <component :is="portIcon(port)" class="size-3" aria-hidden="true" />
                      {{ t(`plugins.portNames.${port}`) }}
                    </Badge>
                  </div>
                </TableCell>
                <TableCell class="hidden align-top text-sm tabular-nums lg:table-cell">
                  {{ plugin.grants.length }}
                </TableCell>
                <TableCell class="align-top">
                  <div class="flex justify-end">
                    <Button variant="ghost" size="icon-sm" as-child>
                      <RouterLink
                        :to="`/plugins/${plugin.name}`"
                        :aria-label="t('plugins.open', { name: plugin.name })"
                      >
                        <ChevronRight aria-hidden="true" />
                      </RouterLink>
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
        <Empty v-else class="border border-dashed">
          <EmptyHeader>
            <EmptyMedia variant="icon"><Puzzle aria-hidden="true" /></EmptyMedia>
            <EmptyTitle>{{ t('plugins.empty') }}</EmptyTitle>
            <EmptyDescription>{{ t('plugins.emptyDetail') }}</EmptyDescription>
          </EmptyHeader>
        </Empty>
      </CardContent>
    </Card>

    <div class="grid gap-6 lg:grid-cols-2">
      <TrustedKeysCard :manages="manages" />
      <SecretsCard :manages="manages" />
    </div>
  </div>
</template>
