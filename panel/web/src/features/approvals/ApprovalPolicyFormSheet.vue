<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { Save, ShieldCheck } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { ApprovalPolicy } from '@/api/generated'
import { putApprovalPolicyMutation } from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import SwitchField from '@/components/SwitchField.vue'
import WindowsField from '@/components/WindowsField.vue'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
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
import { policyForm, policyInput, RESOURCE_KINDS } from './presentation'

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
              <label
                v-for="kind in RESOURCE_KINDS"
                :key="kind"
                class="flex items-center gap-2 text-sm"
              >
                <Checkbox
                  :id="`policy-kind-${kind}`"
                  :model-value="form.resources.includes(kind)"
                  @update:model-value="toggle(form.resources, kind, $event)"
                />
                {{ t(`approvals.kinds.${kind}`) }}
              </label>
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

          <WindowsField
            id="policy-windows"
            v-model="form.windows"
            :legend="t('approvals.windows')"
            :hint="t('approvals.windowsHint')"
          />

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
