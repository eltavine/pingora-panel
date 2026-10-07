<script setup lang="ts">
import { ArrowDown, ArrowUp, Plus, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { Button } from '@/components/ui/button'
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
import { rewriteIcons } from './presentation'
import {
  newRewrite,
  REWRITE_FLAGS,
  REWRITE_KINDS,
  rewriteProblem,
  type RewriteForm,
  type RewriteKind,
} from './rewrites'

const rules = defineModel<RewriteForm[]>({ required: true })
const props = defineProps<{ idPrefix: string }>()

const { t } = useI18n()
const id = (index: number, field: string) => `${props.idPrefix}-${index}-${field}`

function add(kind: RewriteKind) {
  rules.value = [...rules.value, newRewrite(kind)]
}

function remove(index: number) {
  rules.value = rules.value.filter((_, at) => at !== index)
}

function move(index: number, by: -1 | 1) {
  const next = [...rules.value]
  const [rule] = next.splice(index, 1)
  if (rule) {
    next.splice(index + by, 0, rule)
    rules.value = next
  }
}
</script>

<template>
  <div class="flex flex-col gap-3">
    <ol v-if="rules.length" class="flex flex-col gap-3">
      <li
        v-for="(rule, index) in rules"
        :key="index"
        class="bg-muted/30 flex flex-col gap-3 rounded-lg border p-3"
        :data-rewrite="rule.kind"
      >
        <div class="flex items-center gap-2">
          <component
            :is="rewriteIcons[rule.kind]"
            class="text-muted-foreground size-4 shrink-0"
            aria-hidden="true"
          />
          <span class="flex-1 text-sm font-medium">
            <span class="text-muted-foreground font-mono text-xs">{{ index + 1 }}.</span>
            {{ t(`routes.rewrites.kinds.${rule.kind}`) }}
          </span>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            :disabled="index === 0"
            :aria-label="t('routes.rewrites.moveUp', { position: index + 1 })"
            @click="move(index, -1)"
          >
            <ArrowUp aria-hidden="true" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            :disabled="index === rules.length - 1"
            :aria-label="t('routes.rewrites.moveDown', { position: index + 1 })"
            @click="move(index, 1)"
          >
            <ArrowDown aria-hidden="true" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            :aria-label="t('routes.rewrites.remove', { position: index + 1 })"
            @click="remove(index)"
          >
            <Trash2 aria-hidden="true" />
          </Button>
        </div>

        <Input
          v-if="rule.kind === 'strip_prefix' || rule.kind === 'add_prefix'"
          :id="id(index, 'prefix')"
          v-model="rule.prefix"
          class="font-mono text-xs"
          autocomplete="off"
          placeholder="/api"
          :aria-label="t('routes.rewrites.prefix')"
        />
        <Input
          v-else-if="rule.kind === 'set_uri'"
          :id="id(index, 'template')"
          v-model="rule.template"
          class="font-mono text-xs"
          autocomplete="off"
          placeholder="/index.php?q=$uri"
          :aria-label="t('routes.rewrites.template')"
        />
        <div v-else class="grid gap-2 sm:grid-cols-[1fr_1fr_8rem]">
          <Input
            :id="id(index, 'pattern')"
            v-model="rule.pattern"
            class="font-mono text-xs"
            autocomplete="off"
            placeholder="^/old/(.*)$"
            :aria-label="t('routes.rewrites.pattern')"
          />
          <Input
            :id="id(index, 'replacement')"
            v-model="rule.replacement"
            class="font-mono text-xs"
            autocomplete="off"
            placeholder="/new/$1"
            :aria-label="t('routes.rewrites.replacement')"
          />
          <Select v-model="rule.flag">
            <SelectTrigger
              :id="id(index, 'flag')"
              class="w-full"
              :aria-label="t('routes.rewrites.flag')"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="flag in REWRITE_FLAGS" :key="flag" :value="flag">
                {{ t(`routes.rewrites.flags.${flag}`) }}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
        <p v-if="rewriteProblem(rule)" class="text-destructive text-xs" role="alert">
          {{ t(rewriteProblem(rule) ?? '') }}
        </p>
      </li>
    </ol>

    <DropdownMenu>
      <DropdownMenuTrigger as-child>
        <Button type="button" variant="outline" size="sm" class="self-start">
          <Plus data-icon="inline-start" aria-hidden="true" />
          {{ t('routes.rewrites.add') }}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" class="w-56">
        <DropdownMenuItem v-for="kind in REWRITE_KINDS" :key="kind" @select="add(kind)">
          <component :is="rewriteIcons[kind]" aria-hidden="true" />
          {{ t(`routes.rewrites.kinds.${kind}`) }}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  </div>
</template>
