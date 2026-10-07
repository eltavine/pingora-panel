<script setup lang="ts">
import { computed } from 'vue'
import { FileDiff, Minus, PencilLine, Plus } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { Plan } from '@/api/generated'

/** What applying a plan changes, against which revision, and its digest. */
const props = defineProps<{ plan: Plan }>()

const { t } = useI18n()
const counts = computed(() => ({
  added: props.plan.resources.filter((item) => item.change === 'added').length,
  changed: props.plan.resources.filter((item) => item.change === 'changed').length,
  removed: props.plan.resources.filter((item) => item.change === 'removed').length,
  files: props.plan.files.length,
}))
</script>

<template>
  <div class="flex flex-col gap-2 text-sm">
    <p>
      {{
        plan.active_revision == null
          ? t('plan.first')
          : t('plan.against', { revision: plan.active_revision })
      }}
    </p>
    <ul class="flex flex-wrap gap-2" :aria-label="t('plan.summary')">
      <li v-if="counts.added" class="flex items-center gap-1.5 rounded-md border px-2 py-1">
        <Plus class="size-4" aria-hidden="true" />{{ t('plan.added', { count: counts.added }) }}
      </li>
      <li v-if="counts.changed" class="flex items-center gap-1.5 rounded-md border px-2 py-1">
        <PencilLine class="size-4" aria-hidden="true" />{{
          t('plan.changed', { count: counts.changed })
        }}
      </li>
      <li v-if="counts.removed" class="flex items-center gap-1.5 rounded-md border px-2 py-1">
        <Minus class="size-4" aria-hidden="true" />{{
          t('plan.removed', { count: counts.removed })
        }}
      </li>
      <li v-if="counts.files" class="flex items-center gap-1.5 rounded-md border px-2 py-1">
        <FileDiff class="size-4" aria-hidden="true" />{{ t('plan.files', counts.files) }}
      </li>
    </ul>
    <p class="text-muted-foreground font-mono text-xs break-all">
      {{ t('plan.digest', { digest: plan.digest }) }}
    </p>
  </div>
</template>
