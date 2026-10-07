<script setup lang="ts">
import { computed } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { useI18n } from 'vue-i18n'
import { listCachePoliciesOptions } from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'

const NONE = '-'

/**
 * The policy's id; empty for none, which a route reads as its site's, and
 * `off` for a route that stays out of the cache.
 */
const model = defineModel<string>({ required: true })
const props = defineProps<{ id: string; hint: string; route?: boolean }>()

const { t } = useI18n()
const policies = useQuery(listCachePoliciesOptions())
const selected = computed({
  get: () => model.value || NONE,
  set: (value: string) => (model.value = value === NONE ? '' : value),
})
</script>

<template>
  <FormField :id="id" :label="t('cache.select.label')" :hint="hint">
    <Select v-model="selected">
      <SelectTrigger :id="id" class="w-full"><SelectValue /></SelectTrigger>
      <SelectContent>
        <SelectItem :value="NONE">
          {{ props.route ? t('cache.select.site') : t('cache.select.none') }}
        </SelectItem>
        <SelectItem v-if="props.route" value="off">{{ t('cache.select.off') }}</SelectItem>
        <SelectItem v-for="item in policies.data.value ?? []" :key="item.id" :value="item.id">
          {{ item.id }}
        </SelectItem>
      </SelectContent>
    </Select>
  </FormField>
</template>
