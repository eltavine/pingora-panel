<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import FormField from '@/components/FormField.vue'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { ACCESS_LOG_FORMATS, invalidFieldLines, type AccessLogForm } from './forms'

const form = defineModel<AccessLogForm>({ required: true })
const props = defineProps<{ idPrefix: string; scope: 'site' | 'route' }>()

const { t } = useI18n()
const inherited = computed(() => t(`sites.accessLog.inherit.${props.scope}`))
const invalid = computed(() => invalidFieldLines(form.value.fields))
const logging = computed(() => form.value.enabled !== 'off')
</script>

<template>
  <fieldset class="flex flex-col gap-4">
    <legend class="mb-1.5 text-sm font-medium">{{ t('sites.accessLog.title') }}</legend>
    <div class="grid gap-4 sm:grid-cols-2">
      <FormField :id="`${idPrefix}-enabled`" :label="t('sites.accessLog.enabled')">
        <Select v-model="form.enabled">
          <SelectTrigger :id="`${idPrefix}-enabled`" class="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="inherit">{{ inherited }}</SelectItem>
            <SelectItem value="on">{{ t('sites.accessLog.on') }}</SelectItem>
            <SelectItem value="off">{{ t('sites.accessLog.off') }}</SelectItem>
          </SelectContent>
        </Select>
      </FormField>
      <FormField v-if="logging" :id="`${idPrefix}-format`" :label="t('sites.accessLog.format')">
        <Select v-model="form.format">
          <SelectTrigger :id="`${idPrefix}-format`" class="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="inherit">{{ inherited }}</SelectItem>
            <SelectItem v-for="format in ACCESS_LOG_FORMATS" :key="format" :value="format">
              {{ t(`sites.accessLog.formats.${format}`) }}
            </SelectItem>
          </SelectContent>
        </Select>
      </FormField>
    </div>
    <FormField
      v-if="logging"
      :id="`${idPrefix}-fields`"
      :label="t('sites.accessLog.fields')"
      :hint="t('sites.accessLog.fieldsHint')"
    >
      <Textarea
        :id="`${idPrefix}-fields`"
        v-model="form.fields"
        rows="3"
        class="font-mono text-xs"
        placeholder="tenant.id = $http_x_tenant"
        spellcheck="false"
        :aria-invalid="invalid.length > 0"
        :aria-describedby="`${idPrefix}-fields-hint`"
      />
      <p v-if="invalid.length > 0" role="alert" class="text-destructive text-xs">
        {{ t('sites.accessLog.invalidLines', { lines: invalid.join(', ') }, invalid.length) }}
      </p>
    </FormField>
  </fieldset>
</template>
