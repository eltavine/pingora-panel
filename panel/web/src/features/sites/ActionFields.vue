<script setup lang="ts">
import { Braces } from '@lucide/vue'
import { useQuery } from '@tanstack/vue-query'
import { useI18n } from 'vue-i18n'
import { listUpstreamsOptions } from '@/api/generated/@tanstack/vue-query.gen'
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
import { REDIRECT_STATUSES, type ActionForm } from './forms'
import { targetProblem } from './rewrites'

const action = defineModel<ActionForm>({ required: true })
const props = defineProps<{ idPrefix: string }>()

const { t } = useI18n()
const upstreams = useQuery(listUpstreamsOptions())
const id = (field: string) => `${props.idPrefix}-${field}`
</script>

<template>
  <div class="flex flex-col gap-4">
    <FormField
      v-if="action.type === 'proxy'"
      :id="id('upstream')"
      :label="t('sites.form.upstream')"
    >
      <Select v-model="action.upstreamId">
        <SelectTrigger :id="id('upstream')" class="w-full">
          <SelectValue :placeholder="t('sites.form.upstreamPlaceholder')" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem
            v-for="upstream in upstreams.data.value ?? []"
            :key="upstream.id"
            :value="upstream.id"
          >
            {{ upstream.name }}
          </SelectItem>
        </SelectContent>
      </Select>
    </FormField>

    <template v-else-if="action.type === 'static'">
      <FormField :id="id('root')" :label="t('sites.form.root')" :hint="t('sites.form.rootHint')">
        <Input :id="id('root')" v-model="action.root" required autocomplete="off" />
      </FormField>
      <FormField
        :id="id('index')"
        :label="t('sites.form.indexFiles')"
        :hint="t('sites.form.tagsHint')"
      >
        <Input :id="id('index')" v-model="action.indexFiles" autocomplete="off" />
      </FormField>
      <SwitchField
        :id="id('spa')"
        v-model="action.spaFallback"
        :label="t('sites.form.spaFallback')"
      />
    </template>

    <template v-else-if="action.type === 'redirect'">
      <FormField
        :id="id('location')"
        :label="t('sites.form.location')"
        :hint="t('sites.form.locationHint')"
      >
        <Input
          :id="id('location')"
          v-model="action.location"
          type="url"
          required
          autocomplete="off"
        />
      </FormField>
      <FormField :id="id('redirect-status')" :label="t('sites.form.redirectStatus')">
        <Select v-model="action.redirectStatus">
          <SelectTrigger :id="id('redirect-status')" class="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem v-for="status in REDIRECT_STATUSES" :key="status" :value="status">
              {{ status }}
            </SelectItem>
          </SelectContent>
        </Select>
      </FormField>
      <SwitchField
        :id="id('preserve')"
        v-model="action.preservePath"
        :label="t('sites.form.preservePath')"
      />
    </template>

    <FormField
      v-else-if="action.type === 'internal_redirect'"
      :id="id('target')"
      :label="t('sites.form.target')"
      :hint="t('sites.form.targetHint')"
    >
      <Input
        :id="id('target')"
        v-model="action.target"
        required
        class="font-mono text-xs"
        autocomplete="off"
        placeholder="/errors$uri"
        :aria-invalid="Boolean(action.target) && Boolean(targetProblem(action.target))"
      />
      <p
        v-if="action.target && targetProblem(action.target)"
        class="text-destructive text-xs"
        role="alert"
      >
        {{ t(targetProblem(action.target) ?? '') }}
      </p>
    </FormField>

    <p
      v-else-if="action.type === 'lua'"
      class="text-muted-foreground flex items-start gap-2 text-sm"
    >
      <Braces class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
      {{ t('sites.form.luaAction') }}
    </p>
    <template v-else>
      <div class="grid gap-4 sm:grid-cols-2">
        <FormField :id="id('respond-status')" :label="t('sites.form.respondStatus')">
          <Input
            :id="id('respond-status')"
            v-model.number="action.respondStatus"
            type="number"
            min="100"
            max="599"
          />
        </FormField>
        <FormField :id="id('retry-after')" :label="t('sites.form.retryAfter')">
          <Input :id="id('retry-after')" v-model.number="action.retryAfter" type="number" min="0" />
        </FormField>
      </div>
      <FormField :id="id('content-type')" :label="t('sites.form.contentType')">
        <Input
          :id="id('content-type')"
          v-model="action.contentType"
          placeholder="text/html; charset=utf-8"
          autocomplete="off"
        />
      </FormField>
      <FormField :id="id('body')" :label="t('sites.form.body')" :hint="t('sites.form.bodyHint')">
        <Textarea :id="id('body')" v-model="action.body" rows="4" class="font-mono text-xs" />
      </FormField>
    </template>
  </div>
</template>
