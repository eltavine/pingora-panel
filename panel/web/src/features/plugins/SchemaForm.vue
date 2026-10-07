<script setup lang="ts">
import { KeyRound } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import FormField from '@/components/FormField.vue'
import SwitchField from '@/components/SwitchField.vue'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import type { Field } from './presentation'

/** Each field's edited value, by key. */
const values = defineModel<Record<string, string | boolean>>({ required: true })

const props = defineProps<{
  /** Prefixes the fields' element IDs. */
  id: string
  fields: Field[]
  /** References secret settings can take. */
  references: string[]
  disabled?: boolean
}>()

const { t } = useI18n()

function fieldId(field: Field) {
  return `${props.id}-${field.key}`
}

function label(field: Field) {
  return field.required ? `${field.title} · ${t('plugins.required')}` : field.title
}

function hint(field: Field) {
  const range =
    field.minimum !== undefined || field.maximum !== undefined
      ? `${field.minimum ?? '…'}–${field.maximum ?? '…'}`
      : ''
  const list = field.kind === 'list' ? t('plugins.listHint') : ''
  return [field.description, range, list].filter(Boolean).join(' · ') || undefined
}

function set(field: Field, value: string | boolean) {
  values.value = { ...values.value, [field.key]: value }
}
</script>

<template>
  <div class="flex flex-col gap-4">
    <datalist :id="`${id}-references`">
      <option v-for="reference in references" :key="reference" :value="reference" />
    </datalist>
    <template v-for="field in fields" :key="field.key">
      <SwitchField
        v-if="field.kind === 'boolean'"
        :id="fieldId(field)"
        :model-value="values[field.key] === true"
        :label="label(field)"
        :hint="hint(field)"
        :disabled="disabled"
        @update:model-value="set(field, $event)"
      />
      <FormField v-else :id="fieldId(field)" :label="label(field)" :hint="hint(field)">
        <div v-if="field.kind === 'secret'" class="relative">
          <KeyRound
            class="text-muted-foreground pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2"
            aria-hidden="true"
          />
          <Input
            :id="fieldId(field)"
            :model-value="String(values[field.key] ?? '')"
            :list="`${id}-references`"
            class="pl-8 font-mono"
            autocomplete="off"
            spellcheck="false"
            :placeholder="t('plugins.secretPlaceholder')"
            :disabled="disabled"
            @update:model-value="set(field, String($event))"
          />
        </div>
        <Select
          v-else-if="field.kind === 'choice'"
          :model-value="String(values[field.key] ?? '')"
          :disabled="disabled"
          @update:model-value="set(field, String($event ?? ''))"
        >
          <SelectTrigger :id="fieldId(field)" class="w-full"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem v-for="choice in field.choices" :key="choice" :value="choice">
              {{ choice }}
            </SelectItem>
          </SelectContent>
        </Select>
        <Textarea
          v-else-if="field.kind === 'list' || field.kind === 'json'"
          :id="fieldId(field)"
          :model-value="String(values[field.key] ?? '')"
          class="min-h-20 font-mono text-xs"
          spellcheck="false"
          :disabled="disabled"
          @update:model-value="set(field, String($event))"
        />
        <Input
          v-else
          :id="fieldId(field)"
          :model-value="String(values[field.key] ?? '')"
          :type="field.kind === 'integer' || field.kind === 'number' ? 'number' : 'text'"
          :step="field.kind === 'integer' ? 1 : undefined"
          :min="field.minimum"
          :max="field.maximum"
          autocomplete="off"
          :disabled="disabled"
          @update:model-value="set(field, String($event))"
        />
      </FormField>
    </template>
  </div>
</template>
