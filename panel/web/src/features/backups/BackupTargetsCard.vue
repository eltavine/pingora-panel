<script setup lang="ts">
import { computed, ref } from 'vue'
import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { ArchiveRestore, CloudDownload, Ellipsis, PackageSearch, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import { deleteTargetArchive, importTargetArchive, type TargetArchiveView } from '@/api/generated'
import { listTargetArchivesOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
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
import { invalidateTagged } from '@/lib/query'

defineProps<{ manages: boolean }>()

const PLUGIN_NAME = /^[a-z0-9][a-z0-9-]*$/

const { t, d, locale } = useI18n()
const format = computed(() => formatters(locale.value))
const client = useQueryClient()

const entered = ref('')
const target = ref('')
const archives = useQuery(
  computed(() => ({
    ...listTargetArchivesOptions({ path: { target: target.value } }),
    enabled: target.value !== '',
  })),
)
function look() {
  const name = entered.value.trim()
  if (PLUGIN_NAME.test(name)) {
    target.value = name
    void archives.refetch()
  }
}

const working = ref(false)
async function fetch(archive: TargetArchiveView) {
  working.value = true
  try {
    const { data } = await importTargetArchive({
      path: { target: target.value, name: archive.name },
      headers: plainHeaders(),
      throwOnError: true,
    })
    toast.success(t('backups.targets.imported', { name: archive.name, files: data.files }))
    void invalidateTagged(client, ['backups'])
  } catch (error) {
    notifyFailure(error, t('backups.targets.importFailed'))
  } finally {
    working.value = false
  }
}

const removing = ref<TargetArchiveView>()
const removeOpen = ref(false)
function askRemove(archive: TargetArchiveView) {
  removing.value = archive
  removeOpen.value = true
}
async function remove() {
  const archive = removing.value
  if (!archive) {
    return
  }
  try {
    await deleteTargetArchive({
      path: { target: target.value, name: archive.name },
      headers: plainHeaders(),
      throwOnError: true,
    })
    toast.success(t('backups.targets.removed', { name: archive.name }))
    void archives.refetch()
  } catch (error) {
    notifyFailure(error, t('backups.targets.removeFailed'))
  }
}
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle class="flex items-center gap-2">
        <CloudDownload class="size-4" aria-hidden="true" />{{ t('backups.targets.title') }}
      </CardTitle>
      <CardDescription>{{ t('backups.targets.description') }}</CardDescription>
    </CardHeader>
    <CardContent class="flex flex-col gap-4">
      <form class="flex flex-wrap items-center gap-2" @submit.prevent="look">
        <Input
          v-model="entered"
          class="max-w-64 font-mono"
          autocomplete="off"
          spellcheck="false"
          :placeholder="t('backups.targets.plugin')"
          :aria-label="t('backups.targets.whose')"
        />
        <Button
          type="submit"
          size="sm"
          variant="outline"
          :disabled="!PLUGIN_NAME.test(entered.trim())"
        >
          <PackageSearch data-icon="inline-start" aria-hidden="true" />
          {{ t('backups.targets.look') }}
        </Button>
      </form>
      <template v-if="target">
        <ApiFailureAlert
          v-if="archives.isError.value"
          :error="archives.error.value"
          retryable
          @retry="archives.refetch()"
        />
        <Skeleton v-else-if="archives.isPending.value" class="h-16 rounded-lg" />
        <div v-else-if="archives.data.value?.length" class="overflow-x-auto">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{{ t('backups.targets.name') }}</TableHead>
                <TableHead class="hidden sm:table-cell">{{ t('backups.size') }}</TableHead>
                <TableHead class="hidden md:table-cell">{{
                  t('backups.targets.created')
                }}</TableHead>
                <TableHead>
                  <span class="sr-only">{{ t('common.actions') }}</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="archive in archives.data.value" :key="archive.name">
                <TableCell class="font-mono text-xs break-all">{{ archive.name }}</TableCell>
                <TableCell class="hidden text-sm tabular-nums sm:table-cell">
                  {{ format.bytes(archive.size_bytes) }}
                </TableCell>
                <TableCell class="hidden text-sm md:table-cell">
                  {{ archive.created_at ? d(new Date(archive.created_at), 'datetime') : '—' }}
                </TableCell>
                <TableCell>
                  <div v-if="manages" class="flex justify-end">
                    <DropdownMenu>
                      <DropdownMenuTrigger as-child>
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          :aria-label="t('backups.targets.actionsFor', { name: archive.name })"
                        >
                          <Ellipsis aria-hidden="true" />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end" class="w-60">
                        <DropdownMenuItem :disabled="working" @select="fetch(archive)">
                          <ArchiveRestore aria-hidden="true" />{{ t('backups.targets.import') }}
                        </DropdownMenuItem>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem variant="destructive" @select="askRemove(archive)">
                          <Trash2 aria-hidden="true" />{{ t('backups.targets.remove') }}
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
        <Empty v-else class="border border-dashed">
          <EmptyHeader>
            <EmptyMedia variant="icon"><CloudDownload aria-hidden="true" /></EmptyMedia>
            <EmptyTitle>{{ t('backups.targets.empty', { target }) }}</EmptyTitle>
          </EmptyHeader>
        </Empty>
      </template>
    </CardContent>
  </Card>

  <ConfirmDialog
    v-model:open="removeOpen"
    :icon="Trash2"
    :title="t('backups.targets.removeTitle', { name: removing?.name ?? '' })"
    :description="t('backups.targets.removeDetail')"
    :confirm-label="t('common.delete')"
    destructive
    @confirm="remove"
  />
</template>
