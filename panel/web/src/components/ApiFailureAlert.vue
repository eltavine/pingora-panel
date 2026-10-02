<script setup lang="ts">
import { computed } from 'vue'
import { CloudOff, OctagonAlert, RotateCw } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { toApiFailure } from '@/lib/api'

const props = defineProps<{
  error: unknown
  retryable?: boolean
}>()

const emit = defineEmits<{ retry: [] }>()
const { t } = useI18n()
const failure = computed(() => toApiFailure(props.error))
</script>

<template>
  <Alert variant="destructive" role="alert">
    <CloudOff v-if="failure.kind === 'unreachable'" aria-hidden="true" />
    <OctagonAlert v-else aria-hidden="true" />
    <AlertTitle>
      <template v-if="failure.kind === 'unreachable'">{{ t('state.unreachableTitle') }}</template>
      <template v-else-if="failure.kind === 'problem'">{{ failure.problem.title }}</template>
      <template v-else>{{ failure.message }}</template>
    </AlertTitle>
    <AlertDescription class="flex flex-col gap-2">
      <template v-if="failure.kind === 'unreachable'">{{ t('state.unreachableDetail') }}</template>
      <template v-else-if="failure.kind === 'problem'">
        <span>{{ failure.problem.detail }}</span>
        <ul v-if="failure.problem.field_errors?.length" class="list-inside list-disc">
          <li v-for="item in failure.problem.field_errors" :key="`${item.code}-${item.message}`">
            <code class="font-mono text-xs">{{ item.code }}</code> {{ item.message }}
          </li>
        </ul>
        <span class="font-mono text-xs">
          {{ failure.problem.code }}
          <template v-if="failure.problem.request_id">
            · {{ t('state.requestId') }} {{ failure.problem.request_id }}
          </template>
        </span>
      </template>
      <div v-if="retryable">
        <Button variant="outline" size="sm" @click="emit('retry')">
          <RotateCw data-icon="inline-start" aria-hidden="true" />
          {{ t('state.retry') }}
        </Button>
      </div>
    </AlertDescription>
  </Alert>
</template>
