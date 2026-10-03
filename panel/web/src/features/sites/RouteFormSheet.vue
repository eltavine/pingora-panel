<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { Save } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { RouteView, SiteView } from '@/api/generated'
import { createRouteMutation, replaceRouteMutation } from '@/api/generated/@tanstack/vue-query.gen'
import ChoiceCards from '@/components/ChoiceCards.vue'
import FormField from '@/components/FormField.vue'
import SwitchField from '@/components/SwitchField.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import {
  changeHeaders,
  notifyFailure,
  plainHeaders,
  useRefreshConfiguration,
} from '@/lib/configuration'
import ActionFields from './ActionFields.vue'
import { ACTION_TYPES, MATCH_KINDS, routeForm, routeInput, type RouteForm } from './forms'
import { actionIcons } from './presentation'

const ANY_HOST = '-'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ site: SiteView; route?: RouteView; priority: number }>()

const { t } = useI18n()
const refresh = useRefreshConfiguration()
const create = useMutation(createRouteMutation())
const replace = useMutation(replaceRouteMutation())
const busy = computed(() => create.isPending.value || replace.isPending.value)

const form = reactive<RouteForm>(routeForm(undefined, 10))
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, routeForm(props.route, props.priority))
  }
})

const actions = computed(() =>
  ACTION_TYPES.map((value) => ({
    value,
    label: t(`routes.actions.${value}`),
    icon: actionIcons[value],
  })),
)
const host = computed({
  get: () => form.host || ANY_HOST,
  set: (value: string) => (form.host = value === ANY_HOST ? '' : value),
})

function saved() {
  toast.success(props.route ? t('common.saved') : t('routes.created'))
  open.value = false
  void refresh()
}

function submit() {
  const onError = (error: unknown) => notifyFailure(error, t('common.changeFailed'))
  if (props.route) {
    replace.mutate(
      {
        path: { id: props.route.id },
        body: routeInput(form, props.route.id),
        headers: changeHeaders(props.route.etag),
      },
      { onSuccess: saved, onError },
    )
  } else {
    create.mutate(
      { path: { id: props.site.id }, body: routeInput(form), headers: plainHeaders() },
      { onSuccess: saved, onError },
    )
  }
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-xl">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>{{ route ? t('routes.edit') : t('routes.add') }}</SheetTitle>
          <SheetDescription>{{ site.name }}</SheetDescription>
        </SheetHeader>

        <div class="flex flex-col gap-4 px-4">
          <div class="grid gap-4 sm:grid-cols-[1fr_8rem]">
            <FormField id="route-name" :label="t('routes.name')">
              <Input id="route-name" v-model="form.name" maxlength="128" autocomplete="off" />
            </FormField>
            <FormField id="route-priority" :label="t('routes.priority')">
              <Input
                id="route-priority"
                v-model.number="form.priority"
                type="number"
                min="0"
                required
              />
            </FormField>
          </div>

          <div class="grid gap-4 sm:grid-cols-[10rem_1fr]">
            <FormField id="route-kind" :label="t('routes.matchKind')">
              <Select v-model="form.kind">
                <SelectTrigger id="route-kind" class="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="kind in MATCH_KINDS" :key="kind" :value="kind">
                    {{ t(`routes.kinds.${kind}`) }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </FormField>
            <FormField id="route-path" :label="t('routes.path')">
              <Input
                id="route-path"
                v-model="form.path"
                required
                class="font-mono text-xs"
                autocomplete="off"
                :placeholder="form.kind === 'regex' ? '^/api/v[0-9]+/' : '/api/'"
              />
            </FormField>
          </div>

          <FormField id="route-host" :label="t('routes.host')">
            <Select v-model="host">
              <SelectTrigger id="route-host" class="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem :value="ANY_HOST">{{ t('common.all') }}</SelectItem>
                <SelectItem
                  v-for="domain in site.domains ?? []"
                  :key="domain.host"
                  :value="domain.host"
                >
                  {{ domain.host }}
                </SelectItem>
              </SelectContent>
            </Select>
          </FormField>

          <SwitchField id="route-enabled" v-model="form.enabled" :label="t('common.enabled')" />

          <div class="flex flex-col gap-1.5">
            <Label>{{ t('routes.action') }}</Label>
            <ChoiceCards
              v-model="form.action.type"
              :label="t('routes.action')"
              :choices="actions"
            />
          </div>
          <ActionFields v-model="form.action" id-prefix="route-action" />
        </div>

        <SheetFooter>
          <Button type="submit" :disabled="busy">
            <Save data-icon="inline-start" aria-hidden="true" />
            {{ route ? t('common.save') : t('common.create') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
