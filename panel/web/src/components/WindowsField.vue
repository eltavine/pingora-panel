<script setup lang="ts">
import { Globe, Plus, Repeat, X } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import {
  blankWindow,
  DAYS,
  describeWindow,
  timeZones,
  windowInput,
  type DayName,
  type WindowForm,
} from '@/lib/windows'

const windows = defineModel<WindowForm[]>({ required: true })
defineProps<{ id: string; legend: string; hint: string }>()

const { t } = useI18n()
const zones = timeZones()

function toggle(window: WindowForm, day: DayName, checked: boolean | 'indeterminate') {
  window.days = checked
    ? DAYS.filter((item) => item === day || window.days.includes(item))
    : window.days.filter((item) => item !== day)
}
</script>

<template>
  <fieldset class="flex min-w-0 flex-col gap-2">
    <legend class="mb-1 text-sm font-medium">{{ legend }}</legend>
    <p class="text-muted-foreground text-xs">{{ hint }}</p>
    <datalist :id="`${id}-zones`">
      <option v-for="zone in zones" :key="zone" :value="zone" />
    </datalist>
    <div
      v-for="(window, index) in windows"
      :key="index"
      class="flex flex-col gap-2 rounded-md border p-3"
    >
      <div v-if="window.custom" class="flex items-start gap-2">
        <Repeat class="text-muted-foreground mt-0.5 size-4 shrink-0" aria-hidden="true" />
        <p class="min-w-0 flex-1 text-sm">
          <span class="text-muted-foreground block text-xs">{{ t('windows.custom') }}</span>
          <code class="break-all">{{
            describeWindow(windowInput(window), (day) => t(`windows.days.${day}`))
          }}</code>
        </p>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          :aria-label="t('windows.remove')"
          :title="t('windows.remove')"
          @click="windows.splice(index, 1)"
        >
          <X aria-hidden="true" />
        </Button>
      </div>
      <template v-else>
        <div class="flex items-start gap-2">
          <div class="flex flex-1 flex-wrap gap-x-3 gap-y-1">
            <label v-for="day in DAYS" :key="day" class="flex items-center gap-1 text-sm">
              <Checkbox
                :id="`${id}-${index}-${day}`"
                :model-value="window.days.includes(day)"
                @update:model-value="toggle(window, day, $event)"
              />
              {{ t(`windows.days.${day}`) }}
            </label>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            class="-mt-1"
            :aria-label="t('windows.remove')"
            :title="t('windows.remove')"
            @click="windows.splice(index, 1)"
          >
            <X aria-hidden="true" />
          </Button>
        </div>
        <div class="flex items-center gap-2">
          <Input
            v-model="window.start"
            type="time"
            :aria-label="t('windows.start')"
            class="min-w-0 flex-1"
            required
          />
          <span aria-hidden="true">–</span>
          <Input
            v-model="window.end"
            type="time"
            :aria-label="t('windows.end')"
            class="min-w-0 flex-1"
            required
          />
        </div>
        <div class="relative">
          <Globe
            class="text-muted-foreground pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2"
            aria-hidden="true"
          />
          <Input
            v-model="window.timeZone"
            :list="`${id}-zones`"
            :aria-label="t('windows.timeZone')"
            :placeholder="t('windows.timeZone')"
            class="pl-8"
            spellcheck="false"
            autocomplete="off"
            required
          />
        </div>
      </template>
    </div>
    <Button
      type="button"
      variant="outline"
      size="sm"
      class="self-start"
      @click="windows.push(blankWindow())"
    >
      <Plus data-icon="inline-start" aria-hidden="true" />
      {{ t('windows.add') }}
    </Button>
  </fieldset>
</template>
