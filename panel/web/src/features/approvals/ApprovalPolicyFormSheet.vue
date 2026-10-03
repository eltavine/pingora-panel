<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { Plus, Save, ShieldCheck, X } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { ApprovalPolicy, Day } from '@/api/generated'
import { putApprovalPolicyMutation } from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import SwitchField from '@/components/SwitchField.vue'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
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
import { Spinner } from '@/components/ui/spinner'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { DAYS, policyForm, policyInput, RESOURCE_KINDS } from './presentation'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ policy?: ApprovalPolicy }>()
const emit = defineEmits<{ saved: [] }>()

const { t } = useI18n()
const save = useMutation(putApprovalPolicyMutation())
const form = reactive(policyForm())
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, policyForm(props.policy))
  }
})
const complete = computed(() => (props.policy?.id ?? form.id.trim()).length > 0)

function toggle(list: string[], value: string, checked: boolean | 'indeterminate') {
  const index = list.indexOf(value)
  if (checked && index < 0) {
    list.push(value)
  } else if (!checked && index >= 0) {
    list.splice(index, 1)
  }
}

function submit() {
  const id = props.policy?.id ?? form.id.trim()
  save.mutate(
    { path: { id }, body: policyInput(form), headers: plainHeaders() },
    {
      onSuccess: () => {
        toast.success(t('approvals.policySaved', { id }))
        open.value = false
        emit('saved')
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>{{
            policy ? t('approvals.editPolicy') : t('approvals.newPolicy')
          }}</SheetTitle>
          <SheetDescription>{{
            policy?.id ?? t('approvals.policiesDescription')
          }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-5 px-4">
          <FormField v-if="!policy" id="policy-id" :label="t('approvals.policyId')">
            <Input
              id="policy-id"
              v-model="form.id"
              autocomplete="off"
              autocapitalize="none"
              spellcheck="false"
              pattern="[A-Za-z0-9._\-]{1,64}"
              required
            />
          </FormField>
          <FormField id="policy-description" :label="t('approvals.policyDescription')">
            <Input id="policy-description" v-model="form.description" maxlength="256" />
          </FormField>

          <fieldset class="flex flex-col gap-2">
            <legend class="mb-1 text-sm font-medium">{{ t('approvals.resources') }}</legend>
            <p class="text-muted-foreground text-xs">{{ t('approvals.resourcesHint') }}</p>
            <div class="grid grid-cols-2 gap-2">
              <div v-for="kind in RESOURCE_KINDS" :key="kind" class="flex items-center gap-2">
                <Checkbox
                  :id="`policy-kind-${kind}`"
                  :model-value="form.resources.includes(kind)"
                  @update:model-value="toggle(form.resources, kind, $event)"
                />
                <Label :for="`policy-kind-${kind}`" class="font-normal">{{
                  t(`approvals.kinds.${kind}`)
                }}</Label>
              </div>
            </div>
          </fieldset>
          <FormField
            id="policy-tags"
            :label="t('approvals.siteTags')"
            :hint="t('approvals.siteTagsHint')"
          >
            <Input id="policy-tags" v-model="form.siteTags" autocomplete="off" spellcheck="false" />
          </FormField>
          <FormField id="policy-risk" :label="t('approvals.minRisk')">
            <Select v-model="form.minRisk">
              <SelectTrigger id="policy-risk" class="w-full"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value="low">{{ t('approvals.anyRisk') }}</SelectItem>
                <SelectItem value="high">{{ t('approvals.highRiskOnly') }}</SelectItem>
              </SelectContent>
            </Select>
          </FormField>

          <fieldset class="flex flex-col gap-2">
            <legend class="mb-1 text-sm font-medium">{{ t('approvals.windows') }}</legend>
            <p class="text-muted-foreground text-xs">{{ t('approvals.windowsHint') }}</p>
            <div
              v-for="(window, index) in form.windows"
              :key="index"
              class="flex flex-col gap-2 rounded-md border p-3"
            >
              <div class="flex flex-wrap gap-x-3 gap-y-1">
                <div v-for="day in DAYS" :key="day" class="flex items-center gap-1">
                  <Checkbox
                    :id="`window-${index}-${day}`"
                    :model-value="window.days.includes(day)"
                    @update:model-value="toggle(window.days as Day[], day, $event)"
                  />
                  <Label :for="`window-${index}-${day}`" class="font-normal">{{
                    t(`approvals.days.${day}`)
                  }}</Label>
                </div>
              </div>
              <div class="flex items-center gap-2">
                <Input
                  v-model="window.start"
                  type="time"
                  :aria-label="t('approvals.windowStart')"
                  class="min-w-0 flex-1"
                  required
                />
                <span aria-hidden="true">–</span>
                <Input
                  v-model="window.end"
                  type="time"
                  :aria-label="t('approvals.windowEnd')"
                  class="min-w-0 flex-1"
                  required
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  :aria-label="t('approvals.removeWindow')"
                  :title="t('approvals.removeWindow')"
                  @click="form.windows.splice(index, 1)"
                >
                  <X aria-hidden="true" />
                </Button>
              </div>
            </div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              class="self-start"
              @click="form.windows.push({ days: [], start: '09:00', end: '18:00' })"
            >
              <Plus data-icon="inline-start" aria-hidden="true" />
              {{ t('approvals.addWindow') }}
            </Button>
          </fieldset>

          <div class="grid grid-cols-2 gap-4">
            <FormField id="policy-approvals" :label="t('approvals.required')">
              <Input
                id="policy-approvals"
                v-model.number="form.approvals"
                type="number"
                min="1"
                max="5"
                required
              />
            </FormField>
            <FormField id="policy-valid" :label="t('approvals.validMinutes')">
              <Input
                id="policy-valid"
                v-model.number="form.validMinutes"
                type="number"
                min="5"
                max="10080"
                required
              />
            </FormField>
          </div>
          <SwitchField
            id="policy-enabled"
            v-model="form.enabled"
            :label="t('approvals.enabled')"
            :hint="t('approvals.enabledHint')"
          />
        </div>
        <SheetFooter>
          <Button type="submit" :disabled="save.isPending.value || !complete">
            <Spinner v-if="save.isPending.value" data-icon="inline-start" />
            <Save v-else-if="policy" data-icon="inline-start" aria-hidden="true" />
            <ShieldCheck v-else data-icon="inline-start" aria-hidden="true" />
            {{ policy ? t('common.save') : t('common.create') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
