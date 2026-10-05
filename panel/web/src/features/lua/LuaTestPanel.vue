<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import {
  Clock,
  FileCode2,
  FlaskConical,
  Globe,
  Network,
  Play,
  ScrollText,
  Unplug,
  Workflow,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { LuaTest, LuaTestResult } from '@/api/generated'
import { testLuaMutation } from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Spinner } from '@/components/ui/spinner'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Textarea } from '@/components/ui/textarea'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { duration, METHODS, outcomeTone, parseHeaders, PHASES, type Phase } from './presentation'

const props = defineProps<{
  /** The code in the editor, which a script test runs. */
  code?: string
  /** The phase the selected script runs in, for a script test. */
  phase?: Phase
}>()

const { t } = useI18n()

const mode = ref<'request' | 'script'>('request')
const form = reactive({
  method: 'GET',
  host: '',
  target: '/',
  headers: '',
  body: '',
  upstreamStatus: '200',
  upstreamBody: '',
  phase: (props.phase ?? 'access') as Phase,
  body_allowed: false,
  upstream_allowed: false,
})
watch(
  () => props.phase,
  (phase) => {
    if (phase) {
      form.phase = phase
    }
  },
)

const parsed = computed(() => parseHeaders(form.headers))
const status = computed(() => Number.parseInt(form.upstreamStatus, 10))
const valid = computed(
  () =>
    form.host.trim() !== '' &&
    form.target.startsWith('/') &&
    parsed.value.invalid.length === 0 &&
    status.value >= 100 &&
    status.value <= 599 &&
    (mode.value === 'request' || (props.code ?? '').trim() !== ''),
)

const run = useMutation(testLuaMutation())
const result = ref<LuaTestResult>()

function test() {
  if (!valid.value || run.isPending.value) {
    return
  }
  const body: LuaTest = {
    request: {
      method: form.method,
      host: form.host.trim(),
      target: form.target,
      headers: parsed.value.headers,
      body: form.body || undefined,
      tls: false,
    },
    upstream: { status: status.value, body: form.upstreamBody || undefined },
    script:
      mode.value === 'script'
        ? {
            code: props.code ?? '',
            phase: form.phase,
            allow: { body: form.body_allowed, upstream: form.upstream_allowed },
          }
        : undefined,
  }
  run.mutate(
    { body, headers: plainHeaders() },
    {
      onSuccess: (data) => {
        result.value = data
      },
      onError: (error) => notifyFailure(error, t('lua.test.failed')),
    },
  )
}

const response = computed(() => result.value?.response)
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle class="flex items-center gap-2">
        <FlaskConical class="size-4" aria-hidden="true" />
        {{ t('lua.test.title') }}
      </CardTitle>
      <CardDescription>{{ t('lua.test.description') }}</CardDescription>
    </CardHeader>
    <CardContent class="flex flex-col gap-4">
      <Tabs v-model="mode">
        <TabsList class="w-full" :aria-label="t('lua.test.mode')">
          <TabsTrigger value="request">
            <Workflow aria-hidden="true" />
            {{ t('lua.test.configured') }}
          </TabsTrigger>
          <TabsTrigger value="script" :disabled="!code">
            <FileCode2 aria-hidden="true" />
            {{ t('lua.test.script') }}
          </TabsTrigger>
        </TabsList>
      </Tabs>

      <form class="grid gap-3 sm:grid-cols-[8rem_1fr_1fr]" @submit.prevent="test">
        <FormField id="lua-test-method" :label="t('lua.test.method')">
          <Select v-model="form.method">
            <SelectTrigger id="lua-test-method" class="w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem v-for="method in METHODS" :key="method" :value="method">
                {{ method }}
              </SelectItem>
            </SelectContent>
          </Select>
        </FormField>
        <FormField id="lua-test-host" :label="t('lua.test.host')">
          <Input id="lua-test-host" v-model="form.host" placeholder="shop.example" required />
        </FormField>
        <FormField id="lua-test-target" :label="t('lua.test.target')">
          <Input id="lua-test-target" v-model="form.target" class="font-mono" />
        </FormField>
        <FormField
          id="lua-test-headers"
          class="sm:col-span-3"
          :label="t('lua.test.headers')"
          :hint="
            parsed.invalid.length
              ? t('lua.test.invalidHeaders', { lines: parsed.invalid.join(', ') })
              : t('lua.test.headersHint')
          "
        >
          <Textarea
            id="lua-test-headers"
            v-model="form.headers"
            rows="3"
            class="font-mono text-xs"
            :aria-invalid="parsed.invalid.length > 0"
          />
        </FormField>
        <FormField id="lua-test-body" class="sm:col-span-3" :label="t('lua.test.body')">
          <Textarea id="lua-test-body" v-model="form.body" rows="2" class="font-mono text-xs" />
        </FormField>
        <FormField id="lua-test-upstream" :label="t('lua.test.upstreamStatus')">
          <Input id="lua-test-upstream" v-model="form.upstreamStatus" inputmode="numeric" />
        </FormField>
        <FormField
          id="lua-test-upstream-body"
          class="sm:col-span-2"
          :label="t('lua.test.upstreamBody')"
        >
          <Input id="lua-test-upstream-body" v-model="form.upstreamBody" class="font-mono" />
        </FormField>
        <template v-if="mode === 'script'">
          <FormField id="lua-test-phase" :label="t('lua.test.phase')">
            <Select v-model="form.phase">
              <SelectTrigger id="lua-test-phase" class="w-full"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem v-for="phase in PHASES" :key="phase" :value="phase">
                  {{ phase }}
                </SelectItem>
              </SelectContent>
            </Select>
          </FormField>
          <div class="flex flex-wrap items-end gap-4 sm:col-span-2">
            <label class="flex items-center gap-2 text-sm">
              <Checkbox v-model="form.body_allowed" />
              {{ t('lua.test.allowBody') }}
            </label>
            <label class="flex items-center gap-2 text-sm">
              <Checkbox v-model="form.upstream_allowed" />
              {{ t('lua.test.allowUpstream') }}
            </label>
          </div>
        </template>
        <div class="sm:col-span-3">
          <Button type="submit" :disabled="!valid || run.isPending.value">
            <Spinner v-if="run.isPending.value" data-icon="inline-start" />
            <Play v-else data-icon="inline-start" aria-hidden="true" />
            {{ t('lua.test.run') }}
          </Button>
        </div>
      </form>

      <section v-if="result" class="flex flex-col gap-3" :aria-label="t('lua.test.results')">
        <div class="text-muted-foreground flex flex-wrap items-center gap-x-4 gap-y-1 text-xs">
          <span class="flex items-center gap-1">
            <Globe class="size-3.5" aria-hidden="true" />
            {{ result.site_id ?? t('lua.test.noSite') }}
            <template v-if="result.route_id"> · {{ result.route_id }}</template>
          </span>
          <span v-if="result.peer" class="flex items-center gap-1">
            <Network class="size-3.5" aria-hidden="true" />
            {{ t('lua.test.peer', { peer: result.peer }) }}
          </span>
          <span>{{ t('lua.test.version', { version: result.draft_version }) }}</span>
        </div>
        <p v-if="result.runs.length === 0" class="text-muted-foreground text-sm">
          {{ t('lua.test.nothingRan') }}
        </p>
        <div v-else class="rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{{ t('lua.test.phase') }}</TableHead>
                <TableHead>{{ t('lua.script') }}</TableHead>
                <TableHead>{{ t('lua.test.outcome') }}</TableHead>
                <TableHead class="text-right">
                  <Clock class="inline size-3.5" aria-hidden="true" />
                  {{ t('lua.test.time') }}
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <template v-for="(item, index) in result.runs" :key="index">
                <TableRow>
                  <TableCell class="font-mono text-xs">{{ item.phase }}</TableCell>
                  <TableCell class="font-mono text-xs">{{ item.script }}</TableCell>
                  <TableCell>
                    <StatusIndicator
                      :tone="outcomeTone(item.outcome)"
                      :label="t(`lua.outcome.${item.outcome}`)"
                    />
                    <p v-if="item.failure" class="text-muted-foreground mt-1 font-mono text-xs">
                      {{ item.failure.kind }}: {{ item.failure.message }}
                    </p>
                  </TableCell>
                  <TableCell class="text-right font-mono text-xs tabular-nums">
                    {{ duration(item.duration_us) }}
                  </TableCell>
                </TableRow>
                <TableRow v-if="item.logs?.length">
                  <TableCell colspan="4" class="bg-muted/40">
                    <ul class="flex flex-col gap-1 font-mono text-xs">
                      <li v-for="(log, line) in item.logs" :key="line" class="flex gap-2">
                        <ScrollText class="mt-0.5 size-3 shrink-0" aria-hidden="true" />
                        <Badge variant="outline" class="h-4 px-1 text-[10px]">{{
                          log.level
                        }}</Badge>
                        <span class="break-all">{{ log.message }}</span>
                      </li>
                    </ul>
                  </TableCell>
                </TableRow>
              </template>
            </TableBody>
          </Table>
        </div>
        <div v-if="result.aborted" class="flex items-center gap-2 text-sm">
          <Unplug class="size-4" aria-hidden="true" />
          {{ t('lua.test.aborted') }}
        </div>
        <div v-else-if="response" class="rounded-lg border p-3">
          <p class="font-mono text-sm font-medium">HTTP {{ response.status }}</p>
          <ul class="text-muted-foreground mt-1 font-mono text-xs">
            <li v-for="(header, index) in response.headers" :key="index">
              {{ header.name }}: {{ header.value }}
            </li>
          </ul>
          <pre
            v-if="response.body"
            class="bg-muted/40 mt-2 max-h-48 overflow-auto rounded p-2 font-mono text-xs whitespace-pre-wrap"
            >{{ response.body }}</pre>
        </div>
      </section>
    </CardContent>
  </Card>
</template>
