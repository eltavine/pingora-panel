<script setup lang="ts">
import { computed } from 'vue'
import { Plus, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
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
import {
  CONDITION_KINDS,
  GROUP_KINDS,
  isGroup,
  isNamed,
  isTested,
  MOST_DEPTH,
  newCondition,
  takesValue,
  TEST_OPERATORS,
  type ConditionForm,
  type ConditionKind,
} from './conditions'
import { conditionIcons } from './presentation'

defineOptions({ name: 'ConditionsEditor' })

const conditions = defineModel<ConditionForm[]>({ required: true })
const props = defineProps<{ idPrefix: string; depth?: number }>()

const { t } = useI18n()
const depth = computed(() => props.depth ?? 1)
const kinds = computed(() =>
  CONDITION_KINDS.filter((kind) => !isGroup(kind) || depth.value < MOST_DEPTH),
)
const leaves = computed(() => kinds.value.filter((kind) => !GROUP_KINDS.includes(kind)))
const groups = computed(() => kinds.value.filter((kind) => GROUP_KINDS.includes(kind)))

function add(kind: ConditionKind) {
  conditions.value = [...conditions.value, newCondition(kind)]
}

function remove(index: number) {
  conditions.value = conditions.value.filter((_, at) => at !== index)
}

function listPlaceholder(kind: ConditionKind): string {
  switch (kind) {
    case 'method':
      return 'GET, HEAD'
    case 'host':
      return 'shop.example, *.shop.example'
    case 'client':
      return '10.0.0.0/8, 2001:db8::1'
    default:
      return 'application/json, text/*'
  }
}
</script>

<template>
  <div class="flex flex-col gap-3">
    <div
      v-for="(condition, index) in conditions"
      :key="index"
      class="bg-muted/30 flex flex-col gap-3 rounded-lg border p-3"
      :data-condition="condition.kind"
    >
      <div class="flex items-center gap-2">
        <component
          :is="conditionIcons[condition.kind]"
          class="text-muted-foreground size-4 shrink-0"
          aria-hidden="true"
        />
        <span class="flex-1 text-sm font-medium">
          {{ t(`routes.conditions.kinds.${condition.kind}`) }}
        </span>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          :aria-label="
            t('routes.conditions.remove', { kind: t(`routes.conditions.kinds.${condition.kind}`) })
          "
          @click="remove(index)"
        >
          <Trash2 aria-hidden="true" />
        </Button>
      </div>

      <template v-if="isGroup(condition.kind)">
        <p class="text-muted-foreground text-xs">
          {{ t(`routes.conditions.groups.${condition.kind}`) }}
        </p>
        <ConditionsEditor
          v-model="condition.children"
          :id-prefix="`${idPrefix}-${index}`"
          :depth="depth + 1"
        />
      </template>

      <div v-else-if="isTested(condition.kind)" class="grid gap-2 sm:grid-cols-[1fr_9rem_1fr]">
        <Input
          v-if="isNamed(condition.kind)"
          :id="`${idPrefix}-${index}-name`"
          v-model="condition.name"
          class="font-mono text-xs"
          autocomplete="off"
          :placeholder="condition.kind === 'header' ? 'x-env' : 'name'"
          :aria-label="t('routes.conditions.name')"
        />
        <Select v-model="condition.op">
          <SelectTrigger
            :id="`${idPrefix}-${index}-op`"
            class="w-full"
            :class="{ 'sm:col-span-2': !isNamed(condition.kind) && !takesValue(condition.op) }"
            :aria-label="t('routes.conditions.test')"
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem v-for="op in TEST_OPERATORS" :key="op" :value="op">
              {{ t(`routes.conditions.ops.${op}`) }}
            </SelectItem>
          </SelectContent>
        </Select>
        <Input
          v-if="takesValue(condition.op)"
          :id="`${idPrefix}-${index}-value`"
          v-model="condition.value"
          class="font-mono text-xs"
          :class="{ 'sm:col-span-2': !isNamed(condition.kind) }"
          autocomplete="off"
          :placeholder="condition.op === 'regex' ? '^(staging|qa)$' : 'staging'"
          :aria-label="
            condition.op === 'regex' ? t('routes.conditions.pattern') : t('routes.conditions.value')
          "
        />
        <label
          v-if="takesValue(condition.op)"
          class="text-muted-foreground flex items-center gap-2 text-xs sm:col-span-3"
        >
          <Checkbox v-model="condition.ignoreCase" />
          {{ t('routes.conditions.ignoreCase') }}
        </label>
      </div>

      <Input
        v-else
        :id="`${idPrefix}-${index}-list`"
        v-model="condition.list"
        class="font-mono text-xs"
        autocomplete="off"
        :placeholder="listPlaceholder(condition.kind)"
        :aria-label="t(`routes.conditions.lists.${condition.kind}`)"
      />
    </div>

    <DropdownMenu>
      <DropdownMenuTrigger as-child>
        <Button type="button" variant="outline" size="sm" class="self-start">
          <Plus data-icon="inline-start" aria-hidden="true" />
          {{ depth === 1 ? t('routes.conditions.add') : t('routes.conditions.addInside') }}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" class="w-56">
        <DropdownMenuItem v-for="kind in leaves" :key="kind" @select="add(kind)">
          <component :is="conditionIcons[kind]" aria-hidden="true" />
          {{ t(`routes.conditions.kinds.${kind}`) }}
        </DropdownMenuItem>
        <template v-if="groups.length">
          <DropdownMenuSeparator />
          <DropdownMenuItem v-for="kind in groups" :key="kind" @select="add(kind)">
            <component :is="conditionIcons[kind]" aria-hidden="true" />
            {{ t(`routes.conditions.kinds.${kind}`) }}
          </DropdownMenuItem>
        </template>
      </DropdownMenuContent>
    </DropdownMenu>
  </div>
</template>
