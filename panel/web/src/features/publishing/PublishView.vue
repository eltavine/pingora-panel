<script setup lang="ts">
import { watch } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import {
  Ban,
  CircleAlert,
  CircleCheck,
  FileJson2,
  Fingerprint,
  GitCommitHorizontal,
  KeyRound,
  PackageCheck,
  Rocket,
  ShieldCheck,
  TriangleAlert,
  Zap,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { statusOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import CopyValue from '@/components/CopyValue.vue'
import PageHeader from '@/components/PageHeader.vue'
import StatusIndicator from '@/components/StatusIndicator.vue'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Spinner } from '@/components/ui/spinner'
import { Textarea } from '@/components/ui/textarea'
import { usePublishWorkflow } from './usePublishWorkflow'

const { t } = useI18n()
const workflow = usePublishWorkflow()
const status = useQuery(statusOptions())

// Prefill the compare-and-swap guard with the hash the gateway reports.
watch(
  () => status.data.value?.active_hash,
  (hash) => {
    if (hash && !workflow.expectedHash.value) {
      workflow.expectedHash.value = hash
    }
  },
  { immediate: true },
)
</script>

<template>
  <div class="flex flex-col gap-6">
    <PageHeader
      :icon="Rocket"
      :title="t('publish.title')"
      :description="t('publish.description')"
    />

    <Card>
      <CardHeader>
        <CardTitle class="flex items-center gap-2">
          <FileJson2 class="size-4" aria-hidden="true" />
          {{ t('publish.document') }}
        </CardTitle>
        <CardDescription>{{ t('publish.documentHint') }}</CardDescription>
      </CardHeader>
      <CardContent class="flex flex-col gap-4">
        <div class="grid max-w-xs gap-2">
          <Label for="schema-version">{{ t('publish.schemaVersion') }}</Label>
          <Input id="schema-version" v-model="workflow.schemaVersion.value" class="font-mono" />
        </div>
        <div class="grid gap-2">
          <Label for="snapshot-document">{{ t('publish.document') }}</Label>
          <Textarea
            id="snapshot-document"
            v-model="workflow.documentText.value"
            rows="14"
            spellcheck="false"
            class="font-mono text-xs"
            :aria-invalid="!workflow.parsed.value.ok && workflow.parsed.value.reason !== ''"
          />
          <p
            v-if="!workflow.parsed.value.ok && workflow.parsed.value.reason"
            class="flex items-center gap-1.5 text-sm"
            role="alert"
          >
            <TriangleAlert class="size-4 shrink-0" aria-hidden="true" />
            {{ t('publish.invalidJson', { reason: workflow.parsed.value.reason }) }}
          </p>
        </div>
      </CardContent>
      <CardFooter class="flex flex-wrap gap-2">
        <Button
          variant="outline"
          :disabled="!workflow.parsed.value.ok || workflow.validate.isPending.value"
          @click="workflow.runValidate"
        >
          <Spinner v-if="workflow.validate.isPending.value" data-icon="inline-start" />
          <ShieldCheck v-else data-icon="inline-start" aria-hidden="true" />
          {{ t('publish.validate') }}
        </Button>
        <Button
          :disabled="!workflow.parsed.value.ok || workflow.prepare.isPending.value"
          @click="workflow.runPrepare"
        >
          <Spinner v-if="workflow.prepare.isPending.value" data-icon="inline-start" />
          <PackageCheck v-else data-icon="inline-start" aria-hidden="true" />
          {{ t('publish.prepare') }}
        </Button>
      </CardFooter>
    </Card>

    <ApiFailureAlert v-if="workflow.validate.error.value" :error="workflow.validate.error.value" />
    <ApiFailureAlert v-if="workflow.prepare.error.value" :error="workflow.prepare.error.value" />

    <Card v-if="workflow.validation.value">
      <CardHeader>
        <CardTitle>
          <StatusIndicator
            :tone="workflow.validation.value.valid ? 'positive' : 'negative'"
            :label="workflow.validation.value.valid ? t('publish.valid') : t('publish.invalid')"
          />
        </CardTitle>
      </CardHeader>
      <CardContent v-if="workflow.validation.value.diagnostics.length">
        <h2 class="text-muted-foreground mb-2 text-sm font-medium">
          {{ t('publish.diagnostics') }}
        </h2>
        <ul class="flex flex-col gap-2">
          <li
            v-for="(item, index) in workflow.validation.value.diagnostics"
            :key="index"
            class="flex items-start gap-2 text-sm"
          >
            <CircleAlert class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
            <div class="min-w-0">
              <code class="font-mono text-xs">{{ item.code }}</code>
              <span class="ml-2">{{ item.message }}</span>
              <p
                v-if="item.resource_id || item.source_span"
                class="text-muted-foreground font-mono text-xs"
              >
                {{ [item.resource_id, item.source_span].filter(Boolean).join(' · ') }}
              </p>
              <p v-if="item.help" class="text-muted-foreground text-xs">{{ item.help }}</p>
            </div>
          </li>
        </ul>
      </CardContent>
    </Card>

    <Card v-if="workflow.prepared.value">
      <CardHeader>
        <CardTitle>
          <StatusIndicator tone="positive" :label="t('publish.prepared')" />
        </CardTitle>
      </CardHeader>
      <CardContent class="flex flex-col gap-4">
        <dl class="grid gap-3 text-sm sm:grid-cols-[auto_1fr]">
          <dt class="text-muted-foreground flex items-center gap-2">
            <GitCommitHorizontal class="size-4" aria-hidden="true" />{{ t('publish.revision') }}
          </dt>
          <dd class="font-mono tabular-nums">{{ workflow.prepared.value.revision_id }}</dd>
          <dt class="text-muted-foreground flex items-center gap-2">
            <KeyRound class="size-4" aria-hidden="true" />{{ t('publish.prepareToken') }}
          </dt>
          <dd class="min-w-0"><CopyValue :value="workflow.prepared.value.prepare_token" /></dd>
          <dt class="text-muted-foreground flex items-center gap-2">
            <Fingerprint class="size-4" aria-hidden="true" />{{ t('publish.contentHash') }}
          </dt>
          <dd class="min-w-0"><CopyValue :value="workflow.prepared.value.content_hash" /></dd>
        </dl>
        <div class="grid gap-2">
          <Label for="expected-hash">{{ t('publish.expectedHash') }}</Label>
          <Input
            id="expected-hash"
            v-model="workflow.expectedHash.value"
            class="font-mono text-xs"
          />
          <p class="text-muted-foreground text-xs">{{ t('publish.expectedHashHint') }}</p>
        </div>
      </CardContent>
      <CardFooter class="flex flex-wrap gap-2">
        <AlertDialog>
          <AlertDialogTrigger as-child>
            <Button :disabled="workflow.activate.isPending.value">
              <Spinner v-if="workflow.activate.isPending.value" data-icon="inline-start" />
              <Zap v-else data-icon="inline-start" aria-hidden="true" />
              {{ t('publish.activate') }}
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle class="flex items-center gap-2">
                <Zap class="size-5" aria-hidden="true" />{{ t('publish.confirmActivateTitle') }}
              </AlertDialogTitle>
              <AlertDialogDescription>
                {{
                  t('publish.confirmActivateDetail', {
                    revision: workflow.prepared.value.revision_id,
                  })
                }}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>{{ t('publish.cancel') }}</AlertDialogCancel>
              <AlertDialogAction @click="workflow.runActivate">{{
                t('publish.activate')
              }}</AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>

        <AlertDialog>
          <AlertDialogTrigger as-child>
            <Button variant="destructive" :disabled="workflow.abort.isPending.value">
              <Spinner v-if="workflow.abort.isPending.value" data-icon="inline-start" />
              <Ban v-else data-icon="inline-start" aria-hidden="true" />
              {{ t('publish.abort') }}
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle class="flex items-center gap-2">
                <TriangleAlert class="size-5" aria-hidden="true" />{{
                  t('publish.confirmAbortTitle')
                }}
              </AlertDialogTitle>
              <AlertDialogDescription>{{ t('publish.confirmAbortDetail') }}</AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>{{ t('publish.cancel') }}</AlertDialogCancel>
              <AlertDialogAction @click="workflow.runAbort">{{
                t('publish.abort')
              }}</AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </CardFooter>
    </Card>

    <ApiFailureAlert v-if="workflow.activate.error.value" :error="workflow.activate.error.value" />
    <ApiFailureAlert v-if="workflow.abort.error.value" :error="workflow.abort.error.value" />

    <Alert v-if="workflow.activated.value" role="status">
      <CircleCheck aria-hidden="true" />
      <AlertTitle>{{
        t('publish.activated', { revision: workflow.activated.value.revision_id })
      }}</AlertTitle>
      <AlertDescription class="flex min-w-0 flex-col gap-1">
        <span class="flex min-w-0 flex-wrap items-center gap-2">
          {{ t('publish.contentHash') }}
          <CopyValue :value="workflow.activated.value.content_hash" />
        </span>
        <span class="flex min-w-0 flex-wrap items-center gap-2">
          {{ t('publish.previousHash') }}
          <CopyValue
            v-if="workflow.activated.value.previous_active_hash"
            :value="workflow.activated.value.previous_active_hash"
          />
          <span v-else>{{ t('state.none') }}</span>
        </span>
      </AlertDescription>
    </Alert>

    <Alert v-if="workflow.aborted.value" role="status">
      <Ban aria-hidden="true" />
      <AlertTitle>{{ t('publish.aborted') }}</AlertTitle>
    </Alert>
  </div>
</template>
