<script setup lang="ts">
import { computed, ref, type Component } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import {
  ArchiveRestore,
  CircleAlert,
  CloudUpload,
  CircleCheck,
  Database,
  DatabaseBackup,
  Download,
  Ellipsis,
  FileCog,
  FolderOpen,
  LoaderCircle,
  RefreshCw,
  ShieldCheck,
  Trash2,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { toast } from 'vue-sonner'
import {
  copyBackup,
  createBackup,
  deleteBackup,
  restoreBackup,
  type BackupContentName,
  type BackupDetails,
} from '@/api/generated'
import { listBackupsOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import PageHeader from '@/components/PageHeader.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
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
import { notifyFailure, plainHeaders, useRefreshConfiguration } from '@/lib/configuration'
import { formatters } from '@/lib/format'
import { useSession } from '@/lib/session'
import BackupTargetsCard from './BackupTargetsCard.vue'
import {
  archiveUrl,
  CONTENTS,
  restoresConfiguration,
  restoresSites,
  sitePath,
  taking,
} from './presentation'

const { t, d, locale } = useI18n()
const format = computed(() => formatters(locale.value))
const router = useRouter()
const refreshConfiguration = useRefreshConfiguration()
const { can } = useSession()
const manages = computed(() => can('backups.manage'))

const listing = useQuery({
  ...listBackupsOptions(),
  refetchInterval: (query) => (taking(query.state.data?.backups ?? []) ? 2000 : false),
})
const backups = computed(() => listing.data.value?.backups ?? [])

const icons: Record<BackupContentName, Component> = {
  configuration: FileCog,
  certificates: ShieldCheck,
  databases: Database,
  sites: FolderOpen,
}

const working = ref(false)

const takeOpen = ref(false)
const chosen = ref<BackupContentName[]>(['configuration', 'certificates'])
const site = ref('')
function choose(content: BackupContentName, on: boolean | 'indeterminate') {
  chosen.value =
    on === true
      ? CONTENTS.filter((item) => item === content || chosen.value.includes(item))
      : chosen.value.filter((item) => item !== content)
}
async function take() {
  working.value = true
  try {
    const path = sitePath(site.value)
    await createBackup({
      body: {
        contents: chosen.value,
        site_path: chosen.value.includes('sites') && path ? path : undefined,
      },
      headers: plainHeaders(),
      throwOnError: true,
    })
    toast.success(t('backups.takingNow'))
    void listing.refetch()
  } catch (error) {
    notifyFailure(error, t('backups.takeFailed'))
  } finally {
    working.value = false
  }
}

const chosenBackup = ref<BackupDetails>()
const restoreSitesOpen = ref(false)
const restorePath = ref('')
function askRestoreSites(backup: BackupDetails) {
  chosenBackup.value = backup
  restorePath.value = backup.site_path ?? ''
  restoreSitesOpen.value = true
}
async function restoreSites() {
  const backup = chosenBackup.value
  const path = sitePath(restorePath.value)
  if (!backup || !path) {
    return
  }
  working.value = true
  try {
    const { data } = await restoreBackup({
      path: { id: backup.id },
      body: { target: 'sites', site_path: path },
      headers: plainHeaders(),
      throwOnError: true,
    })
    if (data.target === 'sites') {
      toast.success(
        t('backups.sitesRestored', { path: data.site_path, count: data.files }, data.files),
      )
    }
  } catch (error) {
    notifyFailure(error, t('backups.restoreFailed'))
  } finally {
    working.value = false
  }
}

const restoreConfigurationOpen = ref(false)
function askRestoreConfiguration(backup: BackupDetails) {
  chosenBackup.value = backup
  restoreConfigurationOpen.value = true
}
async function restoreConfiguration() {
  const backup = chosenBackup.value
  if (!backup) {
    return
  }
  working.value = true
  try {
    const { data } = await restoreBackup({
      path: { id: backup.id },
      body: { target: 'configuration' },
      headers: plainHeaders(),
      throwOnError: true,
    })
    void refreshConfiguration()
    if (data.target === 'configuration') {
      toast.success(t('backups.configurationRestored', { version: data.draft_version }), {
        action: { label: t('backups.review'), onClick: () => void router.push('/config') },
      })
    }
  } catch (error) {
    notifyFailure(error, t('backups.restoreFailed'))
  } finally {
    working.value = false
  }
}

const copyOpen = ref(false)
const copyTarget = ref('')
function askCopy(backup: BackupDetails) {
  chosenBackup.value = backup
  copyOpen.value = true
}
async function copy() {
  const backup = chosenBackup.value
  const target = copyTarget.value.trim()
  if (!backup || !target) {
    return
  }
  working.value = true
  try {
    const { data } = await copyBackup({
      path: { id: backup.id },
      body: { target },
      headers: plainHeaders(),
      throwOnError: true,
    })
    toast.success(t('backups.copied', { target, name: data.name }))
  } catch (error) {
    notifyFailure(error, t('backups.copyFailed'))
  } finally {
    working.value = false
  }
}

const removeOpen = ref(false)
function askRemove(backup: BackupDetails) {
  chosenBackup.value = backup
  removeOpen.value = true
}
async function remove() {
  const backup = chosenBackup.value
  if (!backup) {
    return
  }
  try {
    await deleteBackup({ path: { id: backup.id }, headers: plainHeaders(), throwOnError: true })
    toast.success(t('backups.removed'))
    void listing.refetch()
  } catch (error) {
    notifyFailure(error, t('backups.removeFailed'))
  }
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="DatabaseBackup"
      :title="t('backups.title')"
      :description="t('backups.description')"
    >
      <template #actions>
        <Button
          variant="outline"
          size="sm"
          :disabled="listing.isFetching.value"
          @click="listing.refetch()"
        >
          <RefreshCw data-icon="inline-start" aria-hidden="true" />
          {{ t('backups.refresh') }}
        </Button>
        <Button v-if="manages" size="sm" @click="takeOpen = true">
          <DatabaseBackup data-icon="inline-start" aria-hidden="true" />
          {{ t('backups.take') }}
        </Button>
      </template>
    </PageHeader>

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
        <div v-else-if="backups.length" class="overflow-x-auto">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{{ t('backups.taken') }}</TableHead>
                <TableHead>{{ t('backups.holds') }}</TableHead>
                <TableHead class="hidden sm:table-cell">{{ t('backups.state') }}</TableHead>
                <TableHead class="hidden md:table-cell">{{ t('backups.size') }}</TableHead>
                <TableHead>
                  <span class="sr-only">{{ t('common.actions') }}</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="backup in backups" :key="backup.id">
                <TableCell class="align-top">
                  <div class="flex flex-col gap-0.5">
                    <span class="text-sm whitespace-nowrap">
                      {{ d(new Date(backup.requested_at), 'datetime') }}
                    </span>
                    <span class="text-muted-foreground text-xs">
                      {{ t('backups.by', { actor: backup.requested_by }) }}
                    </span>
                    <span class="text-muted-foreground text-xs sm:hidden">
                      {{ t(`backups.states.${backup.state}`) }}
                      <template v-if="backup.failure">{{
                        ` · ${backup.failure.message}`
                      }}</template>
                    </span>
                  </div>
                </TableCell>
                <TableCell class="align-top">
                  <div class="flex flex-wrap gap-1">
                    <Badge
                      v-for="content in backup.contents"
                      :key="content"
                      variant="secondary"
                      class="gap-1"
                    >
                      <component :is="icons[content]" class="size-3" aria-hidden="true" />
                      {{ t(`backups.contents.${content}`) }}
                      <span v-if="content === 'sites' && backup.site_path" class="font-mono">
                        {{ ` · ${backup.site_path}` }}
                      </span>
                    </Badge>
                  </div>
                </TableCell>
                <TableCell class="hidden align-top sm:table-cell">
                  <Badge
                    :variant="backup.state === 'failed' ? 'destructive' : 'outline'"
                    class="gap-1"
                    :title="backup.failure?.message"
                  >
                    <LoaderCircle
                      v-if="backup.state === 'pending' || backup.state === 'running'"
                      class="size-3 animate-spin motion-reduce:animate-none"
                      aria-hidden="true"
                    />
                    <CircleCheck
                      v-else-if="backup.state === 'completed'"
                      class="size-3"
                      aria-hidden="true"
                    />
                    <CircleAlert v-else class="size-3" aria-hidden="true" />
                    {{ t(`backups.states.${backup.state}`) }}
                  </Badge>
                  <p
                    v-if="backup.failure"
                    class="text-muted-foreground mt-1 max-w-64 text-xs break-words"
                  >
                    {{ backup.failure.message }}
                  </p>
                </TableCell>
                <TableCell
                  class="hidden align-top text-sm whitespace-nowrap tabular-nums md:table-cell"
                >
                  {{ backup.state === 'completed' ? format.bytes(backup.size_bytes) : '—' }}
                </TableCell>
                <TableCell class="align-top">
                  <div v-if="manages" class="flex justify-end">
                    <DropdownMenu>
                      <DropdownMenuTrigger as-child>
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          :aria-label="
                            t('backups.actionsFor', {
                              time: d(new Date(backup.requested_at), 'datetime'),
                            })
                          "
                        >
                          <Ellipsis aria-hidden="true" />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end" class="w-64">
                        <DropdownMenuItem v-if="backup.state === 'completed'" as-child>
                          <a :href="archiveUrl(backup)" download>
                            <Download aria-hidden="true" />{{ t('backups.download') }}
                          </a>
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          v-if="restoresSites(backup)"
                          @select="askRestoreSites(backup)"
                        >
                          <FolderOpen aria-hidden="true" />{{ t('backups.restoreSites') }}
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          v-if="restoresConfiguration(backup)"
                          @select="askRestoreConfiguration(backup)"
                        >
                          <ArchiveRestore aria-hidden="true" />{{
                            t('backups.restoreConfiguration')
                          }}
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          v-if="backup.state === 'completed'"
                          @select="askCopy(backup)"
                        >
                          <CloudUpload aria-hidden="true" />{{ t('backups.copy') }}
                        </DropdownMenuItem>
                        <DropdownMenuSeparator v-if="backup.state === 'completed'" />
                        <DropdownMenuItem
                          variant="destructive"
                          :disabled="backup.state === 'pending' || backup.state === 'running'"
                          @select="askRemove(backup)"
                        >
                          <Trash2 aria-hidden="true" />{{ t('backups.remove') }}
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
            <EmptyMedia variant="icon"><DatabaseBackup aria-hidden="true" /></EmptyMedia>
            <EmptyTitle>{{ t('backups.empty') }}</EmptyTitle>
            <EmptyDescription>{{ t('backups.emptyDetail') }}</EmptyDescription>
          </EmptyHeader>
        </Empty>
      </CardContent>
    </Card>

    <ConfirmDialog
      v-model:open="takeOpen"
      :icon="DatabaseBackup"
      :title="t('backups.takeTitle')"
      :description="t('backups.takeDetail')"
      :confirm-label="t('backups.take')"
      :busy="working || !chosen.length"
      @confirm="take"
    >
      <fieldset class="flex flex-col gap-3">
        <legend class="sr-only">{{ t('backups.holds') }}</legend>
        <label v-for="content in CONTENTS" :key="content" class="flex items-start gap-3 text-sm">
          <Checkbox
            :model-value="chosen.includes(content)"
            class="mt-0.5"
            @update:model-value="choose(content, $event)"
          />
          <component :is="icons[content]" class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          <span class="flex flex-col gap-0.5">
            <span class="font-medium">{{ t(`backups.contents.${content}`) }}</span>
            <span class="text-muted-foreground text-xs">{{
              t(`backups.explained.${content}`)
            }}</span>
          </span>
        </label>
        <Input
          v-if="chosen.includes('sites')"
          v-model="site"
          class="font-mono"
          autocomplete="off"
          :placeholder="t('backups.allSites')"
          :aria-label="t('backups.siteDirectory')"
        />
      </fieldset>
    </ConfirmDialog>

    <ConfirmDialog
      v-model:open="restoreSitesOpen"
      :icon="FolderOpen"
      :title="t('backups.restoreSitesTitle')"
      :description="t('backups.restoreSitesDetail')"
      :confirm-label="t('backups.restore')"
      :busy="working || !sitePath(restorePath)"
      @confirm="restoreSites"
    >
      <Input
        v-model="restorePath"
        class="font-mono"
        autocomplete="off"
        placeholder="shop"
        :aria-label="t('backups.siteDirectory')"
      />
    </ConfirmDialog>

    <ConfirmDialog
      v-model:open="restoreConfigurationOpen"
      :icon="ArchiveRestore"
      :title="t('backups.restoreConfigurationTitle')"
      :description="t('backups.restoreConfigurationDetail')"
      :confirm-label="t('backups.restore')"
      :busy="working"
      @confirm="restoreConfiguration"
    />

    <BackupTargetsCard :manages="manages" />

    <ConfirmDialog
      v-model:open="copyOpen"
      :icon="CloudUpload"
      :title="t('backups.copyTitle')"
      :description="t('backups.copyDetail')"
      :confirm-label="t('backups.copyConfirm')"
      :busy="working || !/^[a-z0-9][a-z0-9-]*$/.test(copyTarget.trim())"
      @confirm="copy"
    >
      <Input
        v-model="copyTarget"
        class="font-mono"
        autocomplete="off"
        spellcheck="false"
        :aria-label="t('backups.targets.plugin')"
        :placeholder="t('backups.targets.plugin')"
      />
    </ConfirmDialog>

    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="t('backups.removeTitle')"
      :description="t('backups.removeDetail')"
      :confirm-label="t('common.delete')"
      destructive
      @confirm="remove"
    />
  </div>
</template>
