<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useQuery, useQueryClient } from '@tanstack/vue-query'
import {
  ArrowLeft,
  BadgeCheck,
  Braces,
  CircleAlert,
  CircleArrowUp,
  CircleCheck,
  Gauge,
  HeartPulse,
  Layers,
  ListChecks,
  Power,
  PowerOff,
  Puzzle,
  ShieldAlert,
  SlidersHorizontal,
  Undo2,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'
import { toast } from 'vue-sonner'
import {
  configurePlugin,
  disablePlugin,
  enablePlugin,
  grantPlugin,
  limitPlugin,
  rollbackPlugin,
  upgradePlugin,
  type PluginLimits,
  type PluginView,
} from '@/api/generated'
import {
  getPluginOptions,
  listPluginSecretsOptions,
  listPluginsOptions,
} from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import FormField from '@/components/FormField.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
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
import { Textarea } from '@/components/ui/textarea'
import { changeHeaders, notifyFailure } from '@/lib/configuration'
import { formatters } from '@/lib/format'
import { invalidateTagged } from '@/lib/query'
import { useSession } from '@/lib/session'
import {
  askedCapabilities,
  canRun,
  editorValue,
  fieldsOf,
  portIcon,
  referenceChoices,
  settingsFrom,
  stateTone,
  targetVersion,
  upgradeChoices,
} from './presentation'
import SchemaForm from './SchemaForm.vue'

const MIB = 1024 * 1024

const { t, d, locale } = useI18n()
const format = computed(() => formatters(locale.value))
const route = useRoute()
const client = useQueryClient()
const { can } = useSession()
const manages = computed(() => can('plugins.manage'))

const name = computed(() => String(route.params.name ?? ''))
const query = useQuery(computed(() => getPluginOptions({ path: { name: name.value } })))
const plugin = computed(() => query.data.value)
const listing = useQuery(listPluginsOptions())
const secrets = useQuery(
  computed(() => ({ ...listPluginSecretsOptions(), enabled: manages.value })),
)

const target = computed(() => (plugin.value ? targetVersion(plugin.value) : undefined))
const fields = computed(() => fieldsOf(target.value?.config_schema))
const asked = computed(() => (plugin.value ? askedCapabilities(plugin.value) : []))
const upgrades = computed(() => (plugin.value ? upgradeChoices(plugin.value) : []))
const references = computed(() =>
  referenceChoices(secrets.data.value ?? [], listing.data.value?.plugins ?? [], name.value),
)

const grants = ref<string[]>([])
const edited = ref<Record<string, string | boolean>>({})
const asJson = ref(false)
const json = ref('{}')
const limits = ref({ memory: 0, cpu: 0, files: 0, concurrency: 0, timeout: 0 })

function reset(view: PluginView | undefined) {
  if (!view) {
    return
  }
  grants.value = [...view.grants]
  const settings = (view.settings ?? {}) as Record<string, unknown>
  edited.value = Object.fromEntries(
    fields.value.map((field) => [field.key, editorValue(field, settings[field.key])]),
  )
  json.value = JSON.stringify(settings, null, 2)
  asJson.value = fields.value.length === 0
  limits.value = {
    memory: Math.round((view.limits.memory_bytes ?? 0) / MIB),
    cpu: view.limits.cpu_seconds ?? 0,
    files: view.limits.open_files ?? 0,
    concurrency: view.limits.concurrency ?? 0,
    timeout: view.limits.call_timeout_ms ?? 0,
  }
}
watch(
  () => plugin.value?.etag,
  () => reset(plugin.value),
  { immediate: true },
)

const working = ref(false)
async function change(run: (etag: string) => Promise<unknown>, done: string) {
  const view = plugin.value
  if (!view) {
    return
  }
  working.value = true
  try {
    await run(view.etag)
    toast.success(done)
    await invalidateTagged(client, ['plugins'])
  } catch (error) {
    notifyFailure(error, t('plugins.changeFailed'))
  } finally {
    working.value = false
  }
}

const path = computed(() => ({ name: name.value }))

const enableOpen = ref(false)
const disableOpen = ref(false)
const rollbackOpen = ref(false)
const upgradeOpen = ref(false)
const upgradeTo = ref('')
function askUpgrade(version: string) {
  upgradeTo.value = version
  upgradeOpen.value = true
}

function enable() {
  const version = target.value?.version
  return change(
    (etag) =>
      enablePlugin({
        path: path.value,
        body: { version },
        headers: changeHeaders(etag),
        throwOnError: true,
      }),
    t('plugins.enabled', { name: name.value }),
  )
}
function disable() {
  return change(
    (etag) => disablePlugin({ path: path.value, headers: changeHeaders(etag), throwOnError: true }),
    t('plugins.disabled', { name: name.value }),
  )
}
function upgrade() {
  const version = upgradeTo.value
  return change(
    (etag) =>
      upgradePlugin({
        path: path.value,
        body: { version },
        headers: changeHeaders(etag),
        throwOnError: true,
      }),
    t('plugins.upgraded', { version }),
  )
}
function rollback() {
  const version = plugin.value?.previous_version ?? ''
  return change(
    (etag) =>
      rollbackPlugin({ path: path.value, headers: changeHeaders(etag), throwOnError: true }),
    t('plugins.rolledBack', { version }),
  )
}

function choose(capability: string, on: boolean | 'indeterminate') {
  grants.value =
    on === true
      ? asked.value.filter((item) => item === capability || grants.value.includes(item))
      : grants.value.filter((item) => item !== capability)
}
function saveGrants() {
  return change(
    (etag) =>
      grantPlugin({
        path: path.value,
        body: { capabilities: grants.value },
        headers: changeHeaders(etag),
        throwOnError: true,
      }),
    t('plugins.granted'),
  )
}

function editedSettings(): Record<string, unknown> | undefined {
  if (asJson.value) {
    try {
      const parsed: unknown = JSON.parse(json.value || '{}')
      if (typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed)) {
        return parsed as Record<string, unknown>
      }
    } catch {
      // Reported below.
    }
    toast.error(t('plugins.invalidJson'))
    return undefined
  }
  try {
    return settingsFrom(
      fields.value,
      edited.value,
      (plugin.value?.settings ?? {}) as Record<string, unknown>,
    )
  } catch {
    const field = fields.value.find((item) => item.kind === 'json')
    toast.error(t('plugins.invalidField', { field: field?.title ?? '' }))
    return undefined
  }
}
function switchEditor() {
  const settings = editedSettings()
  if (!settings) {
    return
  }
  json.value = JSON.stringify(settings, null, 2)
  edited.value = Object.fromEntries(
    fields.value.map((field) => [field.key, editorValue(field, settings[field.key])]),
  )
  asJson.value = !asJson.value
}
function saveSettings() {
  const settings = editedSettings()
  if (!settings) {
    return
  }
  return change(
    (etag) =>
      configurePlugin({
        path: path.value,
        body: settings,
        headers: changeHeaders(etag),
        throwOnError: true,
      }),
    t('plugins.configured'),
  )
}

function saveLimits() {
  const body: PluginLimits = {
    memory_bytes: Math.max(0, Math.round(Number(limits.value.memory) || 0)) * MIB,
    cpu_seconds: Math.max(0, Math.round(Number(limits.value.cpu) || 0)),
    open_files: Math.max(0, Math.round(Number(limits.value.files) || 0)),
    concurrency: Math.max(0, Math.round(Number(limits.value.concurrency) || 0)),
    call_timeout_ms: Math.max(0, Math.round(Number(limits.value.timeout) || 0)),
  }
  return change(
    (etag) =>
      limitPlugin({ path: path.value, body, headers: changeHeaders(etag), throwOnError: true }),
    t('plugins.limited'),
  )
}

const effective = computed(() => plugin.value?.effective_limits)
const limitFields = computed(() => [
  {
    key: 'memory' as const,
    label: t('plugins.limitsFields.memory'),
    value: effective.value ? format.value.bytes(effective.value.memory_bytes ?? 0) : '',
  },
  {
    key: 'cpu' as const,
    label: t('plugins.limitsFields.cpu'),
    value: effective.value ? String(effective.value.cpu_seconds || '∞') : '',
  },
  {
    key: 'files' as const,
    label: t('plugins.limitsFields.files'),
    value: effective.value ? String(effective.value.open_files) : '',
  },
  {
    key: 'concurrency' as const,
    label: t('plugins.limitsFields.concurrency'),
    value: effective.value ? String(effective.value.concurrency) : '',
  },
  {
    key: 'timeout' as const,
    label: t('plugins.limitsFields.timeout'),
    value: effective.value ? String(effective.value.call_timeout_ms) : '',
  },
])

function capabilityKey(capability: string) {
  return capability.replace(/-/g, '_')
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <RouterLink
      to="/plugins"
      class="text-muted-foreground hover:text-foreground inline-flex w-fit items-center gap-1 text-sm"
    >
      <ArrowLeft class="size-4" aria-hidden="true" />{{ t('plugins.back') }}
    </RouterLink>

    <ApiFailureAlert
      v-if="query.isError.value && !plugin"
      :error="query.error.value"
      retryable
      @retry="query.refetch()"
    />
    <Skeleton
      v-else-if="!plugin"
      class="h-40 rounded-lg"
      aria-busy="true"
      :aria-label="t('state.loading')"
    />
    <template v-else>
      <PageHeader :icon="Puzzle" :title="plugin.name" :description="target?.description">
        <template #actions>
          <StatusIndicator
            :tone="stateTone(plugin.state)"
            :label="t(`plugins.states.${plugin.state}`)"
          />
          <template v-if="manages">
            <Button
              v-if="plugin.state === 'disabled'"
              size="sm"
              :disabled="working || !target || !canRun(target)"
              @click="enableOpen = true"
            >
              <Power data-icon="inline-start" aria-hidden="true" />{{ t('plugins.enable') }}
            </Button>
            <Button
              v-else
              size="sm"
              variant="outline"
              :disabled="working"
              @click="disableOpen = true"
            >
              <PowerOff data-icon="inline-start" aria-hidden="true" />{{ t('plugins.disable') }}
            </Button>
            <DropdownMenu v-if="upgrades.length && plugin.active_version">
              <DropdownMenuTrigger as-child>
                <Button size="sm" variant="outline" :disabled="working">
                  <CircleArrowUp data-icon="inline-start" aria-hidden="true" />
                  {{ t('plugins.upgrade') }}
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem
                  v-for="version in upgrades"
                  :key="version.version"
                  @select="askUpgrade(version.version)"
                >
                  <CircleArrowUp aria-hidden="true" />
                  {{ t('plugins.upgradeTo', { version: version.version }) }}
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
            <Button
              v-if="plugin.previous_version"
              size="sm"
              variant="outline"
              :disabled="working"
              @click="rollbackOpen = true"
            >
              <Undo2 data-icon="inline-start" aria-hidden="true" />{{ t('plugins.rollback') }}
            </Button>
          </template>
        </template>
      </PageHeader>

      <Alert v-if="plugin.error" variant="destructive">
        <CircleAlert aria-hidden="true" />
        <AlertTitle>{{ t('plugins.notRunning') }}</AlertTitle>
        <AlertDescription class="break-words">{{ plugin.error }}</AlertDescription>
      </Alert>

      <div class="grid gap-6 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle class="flex items-center gap-2">
              <HeartPulse class="size-4" aria-hidden="true" />{{ t('plugins.healthTitle') }}
            </CardTitle>
          </CardHeader>
          <CardContent>
            <dl v-if="plugin.health" class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
              <dt class="text-muted-foreground">{{ t('plugins.healthFields.status') }}</dt>
              <dd>
                <StatusIndicator
                  :tone="plugin.health.status === 'serving' ? 'positive' : 'negative'"
                  :label="t(`plugins.healthStates.${plugin.health.status}`)"
                />
              </dd>
              <dt class="text-muted-foreground">{{ t('plugins.healthFields.version') }}</dt>
              <dd class="font-mono">{{ plugin.health.version }}</dd>
              <dt class="text-muted-foreground">{{ t('plugins.healthFields.started') }}</dt>
              <dd>{{ d(new Date(plugin.health.started_at), 'datetime') }}</dd>
              <dt class="text-muted-foreground">{{ t('plugins.healthFields.checked') }}</dt>
              <dd>
                {{
                  plugin.health.checked_at
                    ? d(new Date(plugin.health.checked_at), 'datetime')
                    : t('plugins.none')
                }}
              </dd>
              <dt class="text-muted-foreground">{{ t('plugins.healthFields.failures') }}</dt>
              <dd class="tabular-nums">{{ plugin.health.failures }}</dd>
              <dt class="text-muted-foreground">{{ t('plugins.healthFields.restarts') }}</dt>
              <dd class="tabular-nums">{{ plugin.health.restarts }}</dd>
              <template v-if="plugin.health.error">
                <dt class="text-muted-foreground">{{ t('plugins.healthFields.error') }}</dt>
                <dd class="break-words">{{ plugin.health.error }}</dd>
              </template>
            </dl>
            <p v-else class="text-muted-foreground text-sm">{{ t('plugins.healthNone') }}</p>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle class="flex items-center gap-2">
              <ListChecks class="size-4" aria-hidden="true" />{{ t('plugins.grantsTitle') }}
            </CardTitle>
            <CardDescription>{{ t('plugins.grantsDetail') }}</CardDescription>
          </CardHeader>
          <CardContent class="flex flex-col gap-3">
            <p v-if="!asked.length" class="text-muted-foreground text-sm">
              {{ t('plugins.noCapabilities') }}
            </p>
            <label
              v-for="capability in asked"
              :key="capability"
              class="flex items-start gap-3 text-sm"
            >
              <Checkbox
                :model-value="grants.includes(capability)"
                :disabled="!manages || working"
                class="mt-0.5"
                @update:model-value="choose(capability, $event)"
              />
              <component
                :is="portIcon(capability)"
                class="mt-0.5 size-4 shrink-0"
                aria-hidden="true"
              />
              <span class="flex flex-col gap-0.5">
                <span class="font-mono font-medium">{{ capability }}</span>
                <span class="text-muted-foreground text-xs">
                  {{ t(`plugins.capabilities.${capabilityKey(capability)}`) }}
                </span>
              </span>
            </label>
            <div v-if="manages && asked.length" class="flex justify-end">
              <Button size="sm" :disabled="working" @click="saveGrants">
                <ListChecks data-icon="inline-start" aria-hidden="true" />
                {{ t('plugins.grantsSave') }}
              </Button>
            </div>
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader class="flex flex-row items-start justify-between gap-4">
          <div class="flex flex-col gap-1.5">
            <CardTitle class="flex items-center gap-2">
              <SlidersHorizontal class="size-4" aria-hidden="true" />
              {{ t('plugins.settingsTitle') }}
            </CardTitle>
            <CardDescription>{{ t('plugins.settingsDetail') }}</CardDescription>
          </div>
          <Button
            v-if="fields.length"
            size="sm"
            variant="ghost"
            :aria-pressed="asJson"
            @click="switchEditor"
          >
            <Braces data-icon="inline-start" aria-hidden="true" />
            {{ asJson ? t('plugins.editForm') : t('plugins.editJson') }}
          </Button>
        </CardHeader>
        <CardContent class="flex flex-col gap-4">
          <p v-if="!fields.length" class="text-muted-foreground text-sm">
            {{ t('plugins.noSchema') }}
          </p>
          <Textarea
            v-if="asJson"
            v-model="json"
            class="min-h-40 font-mono text-xs"
            spellcheck="false"
            :aria-label="t('plugins.settingsTitle')"
            :disabled="!manages || working"
          />
          <SchemaForm
            v-else
            id="plugin-settings"
            v-model="edited"
            :fields="fields"
            :references="references"
            :disabled="!manages || working"
          />
          <div v-if="manages" class="flex justify-end">
            <Button size="sm" :disabled="working" @click="saveSettings">
              <SlidersHorizontal data-icon="inline-start" aria-hidden="true" />
              {{ t('plugins.settingsSave') }}
            </Button>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle class="flex items-center gap-2">
            <Gauge class="size-4" aria-hidden="true" />{{ t('plugins.limitsTitle') }}
          </CardTitle>
          <CardDescription>
            {{ t('plugins.limitsDetail') }}
            <template v-if="listing.data.value && !listing.data.value.limits_enforced">
              {{ t('plugins.host.notEnforced') }}
            </template>
          </CardDescription>
        </CardHeader>
        <CardContent class="flex flex-col gap-4">
          <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-5">
            <FormField
              v-for="field in limitFields"
              :id="`plugin-limit-${field.key}`"
              :key="field.key"
              :label="field.label"
              :hint="field.value ? t('plugins.effective', { value: field.value }) : undefined"
            >
              <Input
                :id="`plugin-limit-${field.key}`"
                v-model.number="limits[field.key]"
                type="number"
                min="0"
                :disabled="!manages || working"
              />
            </FormField>
          </div>
          <div v-if="manages" class="flex justify-end">
            <Button size="sm" :disabled="working" @click="saveLimits">
              <Gauge data-icon="inline-start" aria-hidden="true" />{{ t('plugins.limitsSave') }}
            </Button>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle class="flex items-center gap-2">
            <Layers class="size-4" aria-hidden="true" />{{ t('plugins.versionsTitle') }}
          </CardTitle>
        </CardHeader>
        <CardContent class="overflow-x-auto">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{{ t('plugins.version') }}</TableHead>
                <TableHead class="hidden md:table-cell">
                  {{ t('plugins.versionsFields.publisher') }}
                </TableHead>
                <TableHead>{{ t('plugins.versionsFields.signedBy') }}</TableHead>
                <TableHead class="hidden sm:table-cell">
                  {{ t('plugins.versionsFields.protocol') }}
                </TableHead>
                <TableHead class="hidden lg:table-cell">{{ t('plugins.ports') }}</TableHead>
                <TableHead>{{ t('plugins.versionsFields.problems') }}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="version in plugin.versions" :key="version.version">
                <TableCell class="align-top">
                  <div class="flex flex-wrap items-center gap-1">
                    <span class="font-mono text-sm">{{ version.version }}</span>
                    <Badge v-if="version.version === plugin.active_version" variant="default">
                      {{ t('plugins.active') }}
                    </Badge>
                    <Badge
                      v-else-if="version.version === plugin.previous_version"
                      variant="outline"
                    >
                      {{ t('plugins.previous') }}
                    </Badge>
                  </div>
                </TableCell>
                <TableCell class="hidden align-top text-sm md:table-cell">
                  {{ version.publisher || t('plugins.none') }}
                </TableCell>
                <TableCell class="align-top text-sm">
                  <span v-if="version.signed_by" class="inline-flex items-center gap-1">
                    <BadgeCheck class="size-4" aria-hidden="true" />{{ version.signed_by }}
                  </span>
                  <span v-else class="text-muted-foreground inline-flex items-center gap-1">
                    <ShieldAlert class="size-4" aria-hidden="true" />{{ t('plugins.untrusted') }}
                  </span>
                </TableCell>
                <TableCell class="hidden align-top text-sm sm:table-cell">
                  <span class="inline-flex items-center gap-1">
                    <CircleCheck v-if="version.compatible" class="size-4" aria-hidden="true" />
                    <CircleAlert v-else class="size-4" aria-hidden="true" />
                    {{ version.protocol_versions.join(', ') }} ·
                    {{ version.compatible ? t('plugins.compatible') : t('plugins.incompatible') }}
                  </span>
                </TableCell>
                <TableCell class="hidden align-top lg:table-cell">
                  <div class="flex flex-wrap gap-1">
                    <component
                      :is="portIcon(port)"
                      v-for="port in version.ports"
                      :key="port"
                      class="size-4"
                      :aria-label="t(`plugins.portNames.${port}`)"
                      role="img"
                    />
                  </div>
                </TableCell>
                <TableCell class="align-top text-sm">
                  <ul v-if="version.problems.length" class="flex max-w-80 flex-col gap-0.5">
                    <li v-for="problem in version.problems" :key="problem" class="break-words">
                      {{ problem }}
                    </li>
                  </ul>
                  <span v-else class="inline-flex items-center gap-1">
                    <CircleCheck class="size-4" aria-hidden="true" />{{ t('plugins.runnable') }}
                  </span>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      <ConfirmDialog
        v-model:open="enableOpen"
        :icon="Power"
        :title="t('plugins.enableTitle', { name: plugin.name })"
        :description="t('plugins.enableDetail', { version: target?.version ?? '' })"
        :confirm-label="t('plugins.enable')"
        :busy="working"
        @confirm="enable"
      />
      <ConfirmDialog
        v-model:open="disableOpen"
        :icon="PowerOff"
        :title="t('plugins.disableTitle', { name: plugin.name })"
        :description="t('plugins.disableDetail')"
        :confirm-label="t('plugins.disable')"
        :busy="working"
        destructive
        @confirm="disable"
      />
      <ConfirmDialog
        v-model:open="upgradeOpen"
        :icon="CircleArrowUp"
        :title="t('plugins.upgradeTitle', { name: plugin.name, version: upgradeTo })"
        :description="t('plugins.upgradeDetail')"
        :confirm-label="t('plugins.upgrade')"
        :busy="working"
        @confirm="upgrade"
      />
      <ConfirmDialog
        v-model:open="rollbackOpen"
        :icon="Undo2"
        :title="
          t('plugins.rollbackTitle', {
            name: plugin.name,
            version: plugin.previous_version ?? '',
          })
        "
        :description="t('plugins.rollbackDetail')"
        :confirm-label="t('plugins.rollback')"
        :busy="working"
        @confirm="rollback"
      />
    </template>
  </div>
</template>
