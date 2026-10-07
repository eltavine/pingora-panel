<script setup lang="ts">
import { ArrowDown, ArrowUp, Plus, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import FormField from '@/components/FormField.vue'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import type { ActionForm } from './forms'
import { cacheIcons, cachePresetIcons, listingIcons } from './presentation'
import {
  AGE_UNITS,
  CACHE_PRESETS,
  cacheRuleProblem,
  isMediaType,
  LISTINGS,
  mediaTypeProblem,
  newCacheRule,
  type AgeUnit,
  type CachePreset,
} from './statics'

const action = defineModel<ActionForm>({ required: true })
const props = defineProps<{ idPrefix: string }>()

const { t } = useI18n()
const id = (field: string) => `${props.idPrefix}-${field}`
const units = Object.keys(AGE_UNITS) as AgeUnit[]

function addType() {
  action.value.mediaTypes = [...action.value.mediaTypes, { extension: '', type: '' }]
}

function removeType(index: number) {
  action.value.mediaTypes = action.value.mediaTypes.filter((_, at) => at !== index)
}

function typeProblem(index: number) {
  const forms = action.value.mediaTypes
  const found = mediaTypeProblem(forms[index]!, forms.slice(0, index))
  return found ? t(found.key, found.values ?? {}) : undefined
}

function addRule(preset: CachePreset) {
  action.value.cache = [...action.value.cache, newCacheRule(preset)]
}

function removeRule(index: number) {
  action.value.cache = action.value.cache.filter((_, at) => at !== index)
}

function moveRule(index: number, by: -1 | 1) {
  const next = [...action.value.cache]
  const [rule] = next.splice(index, 1)
  if (rule) {
    next.splice(index + by, 0, rule)
    action.value.cache = next
  }
}

function ruleProblem(index: number) {
  const rules = action.value.cache
  const found = cacheRuleProblem(rules[index]!, rules.slice(0, index))
  return found ? t(found.key, found.values ?? {}) : undefined
}
</script>

<template>
  <div class="flex flex-col gap-4">
    <FormField
      :id="id('listing')"
      :label="t('sites.statics.listing')"
      :hint="t('sites.statics.listingHint')"
    >
      <Select v-model="action.listing">
        <SelectTrigger :id="id('listing')" class="w-full">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem v-for="listing in LISTINGS" :key="listing" :value="listing">
            <component :is="listingIcons[listing]" aria-hidden="true" />
            {{ t(`sites.statics.listings.${listing}`) }}
          </SelectItem>
        </SelectContent>
      </Select>
    </FormField>

    <fieldset class="flex flex-col gap-2">
      <legend class="text-sm font-medium">{{ t('sites.statics.mediaTypes') }}</legend>
      <p class="text-muted-foreground text-xs">{{ t('sites.statics.mediaTypesHint') }}</p>
      <ul v-if="action.mediaTypes.length" class="flex flex-col gap-2">
        <li v-for="(entry, index) in action.mediaTypes" :key="index" class="flex flex-col gap-1">
          <div class="grid grid-cols-[7rem_1fr_auto] gap-2">
            <Input
              :id="id(`type-${index}-extension`)"
              v-model="entry.extension"
              class="font-mono text-xs"
              autocomplete="off"
              placeholder="wasm"
              :aria-label="t('sites.statics.extension')"
            />
            <Input
              :id="id(`type-${index}-type`)"
              v-model="entry.type"
              class="font-mono text-xs"
              autocomplete="off"
              placeholder="application/wasm"
              :aria-label="t('sites.statics.mediaType')"
            />
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              :aria-label="t('sites.statics.removeType', { position: index + 1 })"
              @click="removeType(index)"
            >
              <Trash2 aria-hidden="true" />
            </Button>
          </div>
          <p v-if="typeProblem(index)" class="text-destructive text-xs" role="alert">
            {{ typeProblem(index) }}
          </p>
        </li>
      </ul>
      <Button type="button" variant="outline" size="sm" class="self-start" @click="addType">
        <Plus data-icon="inline-start" aria-hidden="true" />
        {{ t('sites.statics.addType') }}
      </Button>
    </fieldset>

    <FormField
      :id="id('default-type')"
      :label="t('sites.statics.defaultType')"
      :hint="t('sites.statics.defaultTypeHint')"
    >
      <Input
        :id="id('default-type')"
        v-model="action.defaultType"
        class="font-mono text-xs"
        autocomplete="off"
        placeholder="application/octet-stream"
        :aria-invalid="action.defaultType.trim() !== '' && !isMediaType(action.defaultType)"
      />
    </FormField>

    <fieldset class="flex flex-col gap-2">
      <legend class="text-sm font-medium">{{ t('sites.statics.cache') }}</legend>
      <p class="text-muted-foreground text-xs">{{ t('sites.statics.cacheHint') }}</p>
      <ol v-if="action.cache.length" class="flex flex-col gap-3">
        <li
          v-for="(rule, index) in action.cache"
          :key="index"
          class="bg-muted/30 flex flex-col gap-3 rounded-lg border p-3"
          :data-cache="rule.mode"
        >
          <div class="flex items-center gap-2">
            <component
              :is="cacheIcons[rule.mode]"
              class="text-muted-foreground size-4 shrink-0"
              aria-hidden="true"
            />
            <span class="flex-1 text-sm font-medium">
              <span class="text-muted-foreground font-mono text-xs">{{ index + 1 }}.</span>
              {{ rule.extensions.trim() || t('sites.statics.everyFile') }}
            </span>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              :disabled="index === 0"
              :aria-label="t('sites.statics.moveUp', { position: index + 1 })"
              @click="moveRule(index, -1)"
            >
              <ArrowUp aria-hidden="true" />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              :disabled="index === action.cache.length - 1"
              :aria-label="t('sites.statics.moveDown', { position: index + 1 })"
              @click="moveRule(index, 1)"
            >
              <ArrowDown aria-hidden="true" />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              :aria-label="t('sites.statics.removeRule', { position: index + 1 })"
              @click="removeRule(index)"
            >
              <Trash2 aria-hidden="true" />
            </Button>
          </div>
          <Input
            :id="id(`cache-${index}-extensions`)"
            v-model="rule.extensions"
            class="font-mono text-xs"
            autocomplete="off"
            :placeholder="t('sites.statics.extensionsPlaceholder')"
            :aria-label="t('sites.statics.extensions')"
          />
          <div class="grid gap-2 sm:grid-cols-[12rem_1fr_8rem]">
            <Select v-model="rule.mode">
              <SelectTrigger
                :id="id(`cache-${index}-mode`)"
                class="w-full"
                :aria-label="t('sites.statics.mode')"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="max_age">{{ t('sites.statics.modes.max_age') }}</SelectItem>
                <SelectItem value="no_cache">{{ t('sites.statics.modes.no_cache') }}</SelectItem>
              </SelectContent>
            </Select>
            <template v-if="rule.mode === 'max_age'">
              <Input
                :id="id(`cache-${index}-age`)"
                v-model="rule.age"
                type="number"
                min="0"
                :aria-label="t('sites.statics.age')"
              />
              <Select v-model="rule.unit">
                <SelectTrigger
                  :id="id(`cache-${index}-unit`)"
                  class="w-full"
                  :aria-label="t('sites.statics.unit')"
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="unit in units" :key="unit" :value="unit">
                    {{ t(`sites.statics.units.${unit}`) }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </template>
          </div>
          <label v-if="rule.mode === 'max_age'" class="flex items-center gap-2 text-sm">
            <Checkbox v-model="rule.immutable" />
            {{ t('sites.statics.immutable') }}
          </label>
          <p v-if="ruleProblem(index)" class="text-destructive text-xs" role="alert">
            {{ ruleProblem(index) }}
          </p>
        </li>
      </ol>
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button type="button" variant="outline" size="sm" class="self-start">
            <Plus data-icon="inline-start" aria-hidden="true" />
            {{ t('sites.statics.addRule') }}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" class="w-64">
          <DropdownMenuItem v-for="preset in CACHE_PRESETS" :key="preset" @select="addRule(preset)">
            <component :is="cachePresetIcons[preset]" aria-hidden="true" />
            {{ t(`sites.statics.presets.${preset}`) }}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </fieldset>
  </div>
</template>
