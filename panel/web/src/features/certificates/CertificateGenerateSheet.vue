<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { Sparkles } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import { createCertificateMutation } from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
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
import { CERTIFICATE_ID, MAX_SELF_SIGNED_DAYS, parseNames, suggestedId } from './presentation'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ taken: readonly string[] }>()
const emit = defineEmits<{ saved: [] }>()

const { t } = useI18n()
const create = useMutation(createCertificateMutation())

const form = reactive({ id: '', names: '', days: 90, idEdited: false })
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, { id: '', names: '', days: 90, idEdited: false })
  }
})
const names = computed(() => parseNames(form.names))
watch(names, (current) => {
  if (!form.idEdited) {
    form.id = suggestedId(current)
  }
})

const idError = computed(() => {
  if (form.id === '') {
    return null
  }
  if (!CERTIFICATE_ID.test(form.id)) {
    return t('certificates.idHint')
  }
  return props.taken.includes(form.id) ? t('certificates.idTaken') : null
})
const daysValid = computed(
  () => Number.isInteger(form.days) && form.days >= 1 && form.days <= MAX_SELF_SIGNED_DAYS,
)

function submit() {
  create.mutate(
    {
      body: { source: 'self_signed', id: form.id, names: names.value, days: form.days },
      headers: plainHeaders(),
    },
    {
      onSuccess: (created) => {
        toast.success(t('certificates.generated', { id: created.id }))
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
          <SheetTitle class="flex items-center gap-2">
            <Sparkles class="size-5" aria-hidden="true" />
            {{ t('certificates.generate') }}
          </SheetTitle>
          <SheetDescription>{{ t('certificates.generateDetail') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-4 px-4">
          <FormField
            id="generate-names"
            :label="t('certificates.names')"
            :hint="t('certificates.namesHint')"
          >
            <Textarea
              id="generate-names"
              v-model="form.names"
              required
              rows="4"
              spellcheck="false"
              class="font-mono text-xs"
              placeholder="intranet.example&#10;*.intranet.example"
            />
          </FormField>
          <FormField
            id="generate-id"
            :label="t('certificates.id')"
            :hint="idError ?? t('certificates.idHint')"
          >
            <Input
              id="generate-id"
              v-model="form.id"
              required
              :aria-invalid="idError !== null"
              class="font-mono text-xs"
              autocomplete="off"
              @input="form.idEdited = true"
            />
          </FormField>
          <FormField
            id="generate-days"
            :label="t('certificates.days')"
            :hint="t('certificates.daysHint')"
          >
            <Input
              id="generate-days"
              v-model.number="form.days"
              type="number"
              min="1"
              :max="MAX_SELF_SIGNED_DAYS"
              required
              class="w-32"
            />
          </FormField>
        </div>
        <SheetFooter>
          <Button
            type="submit"
            :disabled="
              create.isPending.value ||
              idError !== null ||
              form.id === '' ||
              names.length === 0 ||
              !daysValid
            "
          >
            <Sparkles data-icon="inline-start" aria-hidden="true" />
            {{ t('certificates.generate') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
