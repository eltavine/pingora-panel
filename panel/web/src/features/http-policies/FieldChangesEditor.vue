<script setup lang="ts">
import { Plus, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import FormField from '@/components/FormField.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  OPERATIONS,
  fieldChangeForm,
  fieldNameInvalid,
  type FieldChangeForm,
  type Operation,
} from './forms'

const changes = defineModel<FieldChangeForm[]>({ required: true })
defineProps<{ idPrefix: string; valuePlaceholder: string }>()

const { t } = useI18n()

function setOperation(change: FieldChangeForm, value: unknown) {
  change.operation = value as Operation
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <div
      v-for="(change, index) in changes"
      :key="index"
      class="flex flex-col gap-3 rounded-md border p-3"
    >
      <div class="grid grid-cols-[7rem_1fr_auto] items-end gap-2">
        <FormField :id="`${idPrefix}-operation-${index}`" :label="t('httpPolicies.operation')">
          <Select
            :model-value="change.operation"
            @update:model-value="setOperation(change, $event)"
          >
            <SelectTrigger :id="`${idPrefix}-operation-${index}`" class="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="operation in OPERATIONS" :key="operation" :value="operation">
                {{ t(`httpPolicies.operations.${operation}`) }}
              </SelectItem>
            </SelectContent>
          </Select>
        </FormField>
        <FormField
          :id="`${idPrefix}-name-${index}`"
          :label="t('httpPolicies.fieldName')"
          :hint="fieldNameInvalid(change) ? t('httpPolicies.fieldNameInvalid') : undefined"
        >
          <Input
            :id="`${idPrefix}-name-${index}`"
            v-model="change.name"
            :aria-invalid="fieldNameInvalid(change)"
            class="font-mono text-xs"
            placeholder="X-Frame-Options"
            autocomplete="off"
          />
        </FormField>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          :aria-label="t('httpPolicies.removeChange')"
          @click="changes.splice(index, 1)"
        >
          <Trash2 aria-hidden="true" />
        </Button>
      </div>
      <FormField
        v-if="change.operation !== 'remove'"
        :id="`${idPrefix}-value-${index}`"
        :label="t('httpPolicies.fieldValue')"
      >
        <Input
          :id="`${idPrefix}-value-${index}`"
          v-model="change.value"
          class="font-mono text-xs"
          :placeholder="valuePlaceholder"
          autocomplete="off"
        />
      </FormField>
    </div>
    <Button
      type="button"
      variant="outline"
      size="sm"
      class="self-start"
      @click="changes.push(fieldChangeForm())"
    >
      <Plus data-icon="inline-start" aria-hidden="true" />
      {{ t('httpPolicies.addChange') }}
    </Button>
  </div>
</template>
