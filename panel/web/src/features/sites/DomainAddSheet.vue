<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { CircleCheck, CircleX, ListPlus, SearchCheck, TriangleAlert } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { DomainCheck, SiteView } from '@/api/generated'
import { addDomainsMutation, checkDomainsMutation } from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Textarea } from '@/components/ui/textarea'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { parseHosts } from './presentation'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ site: SiteView }>()
const emit = defineEmits<{ saved: [site: SiteView] }>()

const { t } = useI18n()
const text = ref('')
const results = ref<DomainCheck[]>([])
const check = useMutation(checkDomainsMutation())
const add = useMutation(addDomainsMutation())

watch(open, (isOpen) => {
  if (isOpen) {
    text.value = ''
    results.value = []
  }
})
watch(text, () => (results.value = []))

const hosts = computed(() => parseHosts(text.value))
const accepted = computed(() =>
  results.value.filter((result) => result.host && !result.error && !result.owner),
)

function verdict(result: DomainCheck) {
  if (result.error) {
    return { icon: CircleX, label: result.error }
  }
  if (result.owner) {
    return { icon: TriangleAlert, label: t('domains.takenBy', { site: result.owner.site_name }) }
  }
  return { icon: CircleCheck, label: t('domains.available') }
}

function runCheck() {
  check.mutate(
    { body: { hosts: hosts.value } },
    {
      onSuccess: (checks) => (results.value = checks),
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

function submit() {
  const hasPrimary = (props.site.domains ?? []).some((domain) => domain.primary)
  const body = accepted.value.map((result, index) => ({
    host: result.host as string,
    enabled: true,
    primary: !hasPrimary && index === 0,
  }))
  add.mutate(
    { path: { id: props.site.id }, body, headers: plainHeaders() },
    {
      onSuccess: (site) => {
        toast.success(t('domains.added', { count: body.length }))
        open.value = false
        emit('saved', site)
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-xl">
      <SheetHeader>
        <SheetTitle>{{ t('domains.addTitle') }}</SheetTitle>
        <SheetDescription>{{ t('domains.addHint') }}</SheetDescription>
      </SheetHeader>
      <div class="flex flex-col gap-4 px-4">
        <FormField id="domain-hosts" :label="t('sites.form.domains')">
          <Textarea
            id="domain-hosts"
            v-model="text"
            rows="6"
            class="font-mono text-xs"
            placeholder="example.com&#10;*.example.com&#10;例子.测试"
          />
        </FormField>
        <div>
          <Button
            variant="outline"
            size="sm"
            :disabled="hosts.length === 0 || check.isPending.value"
            @click="runCheck"
          >
            <SearchCheck data-icon="inline-start" aria-hidden="true" />
            {{ t('domains.check') }}
          </Button>
        </div>
        <section v-if="results.length > 0" class="flex flex-col gap-2">
          <h3 class="text-sm font-medium">{{ t('domains.checkTitle') }}</h3>
          <ul class="divide-y rounded-md border">
            <li
              v-for="result in results"
              :key="result.input"
              class="flex items-start gap-3 px-3 py-2 text-sm"
            >
              <component
                :is="verdict(result).icon"
                class="mt-0.5 size-4 shrink-0"
                aria-hidden="true"
              />
              <div class="flex min-w-0 flex-col">
                <span class="truncate font-mono text-xs">{{ result.host ?? result.input }}</span>
                <span
                  v-if="result.unicode_host && result.unicode_host !== result.host"
                  class="text-muted-foreground text-xs"
                >
                  {{ result.unicode_host }}
                </span>
                <span class="text-muted-foreground text-xs">{{ verdict(result).label }}</span>
              </div>
              <Badge v-if="result.wildcard" variant="outline" class="ml-auto">
                {{ t('domains.wildcard') }}
              </Badge>
            </li>
          </ul>
        </section>
      </div>
      <SheetFooter>
        <Button :disabled="accepted.length === 0 || add.isPending.value" @click="submit">
          <ListPlus data-icon="inline-start" aria-hidden="true" />
          {{ t('domains.add') }}
          <template v-if="accepted.length > 0">({{ accepted.length }})</template>
        </Button>
      </SheetFooter>
    </SheetContent>
  </Sheet>
</template>
