<script setup lang="ts">
import { FileQuestionMark, Plus, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import FormField from '@/components/FormField.vue'
import SwitchField from '@/components/SwitchField.vue'
import { Button } from '@/components/ui/button'
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
import { Textarea } from '@/components/ui/textarea'
import {
  newPage,
  PAGE_KINDS,
  PAGE_PRESETS,
  PAGE_REDIRECT_STATUSES,
  pageProblem,
  type PagesForm,
} from './pages'
import { pageIcons, presetIcons } from './presentation'

const form = defineModel<PagesForm>({ required: true })
const props = defineProps<{ idPrefix: string }>()

const { t } = useI18n()
const id = (index: number, field: string) => `${props.idPrefix}-${index}-${field}`

function add(status?: number) {
  form.value.pages = [...form.value.pages, newPage(status)]
}

function remove(index: number) {
  form.value.pages = form.value.pages.filter((_, at) => at !== index)
}

function problem(index: number) {
  const pages = form.value.pages
  const found = pageProblem(pages[index]!, pages.slice(0, index))
  return found ? t(found.key, found.values ?? {}) : undefined
}
</script>

<template>
  <div class="flex flex-col gap-3">
    <ul v-if="form.pages.length" class="flex flex-col gap-3">
      <li
        v-for="(page, index) in form.pages"
        :key="index"
        class="bg-muted/30 flex flex-col gap-3 rounded-lg border p-3"
        :data-page="page.kind"
      >
        <div class="flex items-center gap-2">
          <component
            :is="pageIcons[page.kind]"
            class="text-muted-foreground size-4 shrink-0"
            aria-hidden="true"
          />
          <span class="flex-1 text-sm font-medium">
            <span class="font-mono">{{ page.statuses || '…' }}</span>
            <span class="text-muted-foreground">
              · {{ t(`sites.errorPages.kinds.${page.kind}`) }}</span
            >
          </span>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            :aria-label="t('sites.errorPages.remove', { position: index + 1 })"
            @click="remove(index)"
          >
            <Trash2 aria-hidden="true" />
          </Button>
        </div>

        <div class="grid gap-2 sm:grid-cols-[1fr_10rem]">
          <Input
            :id="id(index, 'statuses')"
            v-model="page.statuses"
            class="font-mono text-xs"
            autocomplete="off"
            inputmode="numeric"
            placeholder="502, 503"
            :aria-label="t('sites.errorPages.statuses')"
          />
          <Select v-model="page.kind">
            <SelectTrigger
              :id="id(index, 'kind')"
              class="w-full"
              :aria-label="t('sites.errorPages.kind')"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="kind in PAGE_KINDS" :key="kind" :value="kind">
                <component :is="pageIcons[kind]" aria-hidden="true" />
                {{ t(`sites.errorPages.kinds.${kind}`) }}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>

        <template v-if="page.kind === 'body'">
          <Textarea
            :id="id(index, 'body')"
            v-model="page.body"
            rows="3"
            class="font-mono text-xs"
            placeholder="<h1>$host is resting</h1>"
            :aria-label="t('sites.errorPages.body')"
          />
          <p class="text-muted-foreground text-xs">{{ t('sites.form.bodyHint') }}</p>
          <Input
            :id="id(index, 'type')"
            v-model="page.contentType"
            class="font-mono text-xs"
            autocomplete="off"
            placeholder="text/html; charset=utf-8"
            :aria-label="t('sites.errorPages.contentType')"
          />
        </template>
        <FormField
          v-else-if="page.kind === 'file'"
          :id="id(index, 'path')"
          :label="t('sites.errorPages.path')"
          :hint="t('sites.errorPages.pathHint')"
        >
          <Input
            :id="id(index, 'path')"
            v-model="page.path"
            class="font-mono text-xs"
            autocomplete="off"
            placeholder="errors/404.html"
          />
        </FormField>
        <div v-else class="grid gap-2 sm:grid-cols-[1fr_8rem]">
          <Input
            :id="id(index, 'location')"
            v-model="page.location"
            class="font-mono text-xs"
            autocomplete="off"
            placeholder="https://$host/"
            :aria-label="t('sites.errorPages.location')"
          />
          <Select v-model="page.redirectStatus">
            <SelectTrigger
              :id="id(index, 'redirect-status')"
              class="w-full"
              :aria-label="t('sites.errorPages.redirectStatus')"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="status in PAGE_REDIRECT_STATUSES" :key="status" :value="status">
                {{ status }}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>

        <FormField
          v-if="page.kind !== 'redirect'"
          :id="id(index, 'status')"
          :label="t('sites.errorPages.status')"
          :hint="t('sites.errorPages.statusHint')"
        >
          <Input
            :id="id(index, 'status')"
            v-model="page.status"
            type="number"
            min="200"
            max="599"
            class="w-28"
          />
        </FormField>
        <p v-if="problem(index)" class="text-destructive text-xs" role="alert">
          {{ problem(index) }}
        </p>
      </li>
    </ul>

    <DropdownMenu>
      <DropdownMenuTrigger as-child>
        <Button type="button" variant="outline" size="sm" class="self-start">
          <Plus data-icon="inline-start" aria-hidden="true" />
          {{ t('sites.errorPages.add') }}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" class="w-60">
        <DropdownMenuItem v-for="status in PAGE_PRESETS" :key="status" @select="add(status)">
          <component :is="presetIcons[status]" aria-hidden="true" />
          {{ t(`sites.errorPages.presets.${status}`) }}
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem @select="add()">
          <FileQuestionMark aria-hidden="true" />
          {{ t('sites.errorPages.presets.other') }}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>

    <SwitchField
      :id="`${idPrefix}-intercept`"
      v-model="form.intercept"
      :label="t('sites.errorPages.intercept')"
      :hint="t('sites.errorPages.interceptHint')"
    />
  </div>
</template>
