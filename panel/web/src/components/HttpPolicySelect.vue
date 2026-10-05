<script setup lang="ts">
import { computed } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { useI18n } from 'vue-i18n'
import { listHttpPoliciesOptions } from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'

const NONE = '-'

/** The policy's id; empty for none. */
const model = defineModel<string>({ required: true })
defineProps<{ id: string; hint: string }>()

const { t } = useI18n()
const policies = useQuery(listHttpPoliciesOptions())
const selected = computed({
  get: () => model.value || NONE,
  set: (value: string) => (model.value = value === NONE ? '' : value),
})
</script>

<template>
  <FormField :id="id" :label="t('httpPolicies.select.label')" :hint="hint">
    <Select v-model="selected">
      <SelectTrigger :id="id" class="w-full"><SelectValue /></SelectTrigger>
      <SelectContent>
        <SelectItem :value="NONE">{{ t('httpPolicies.select.none') }}</SelectItem>
        <SelectItem v-for="item in policies.data.value ?? []" :key="item.id" :value="item.id">
          {{ item.id }}
        </SelectItem>
      </SelectContent>
    </Select>
  </FormField>
</template>
