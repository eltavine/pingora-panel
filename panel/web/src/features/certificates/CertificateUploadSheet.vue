<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { CircleCheck, FileUp, ScanSearch, TriangleAlert, Upload } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { CertificateView, InspectedCertificate } from '@/api/generated'
import {
  createCertificateMutation,
  inspectCertificateMutation,
  replaceCertificateMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
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
import { toApiFailure } from '@/lib/api'
import { changeHeaders, notifyFailure, plainHeaders } from '@/lib/configuration'
import { CERTIFICATE_ID } from './presentation'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ certificate?: CertificateView; taken: readonly string[] }>()
const emit = defineEmits<{ saved: [] }>()

const { t, d } = useI18n()
const create = useMutation(createCertificateMutation())
const replace = useMutation(replaceCertificateMutation())
const inspect = useMutation(inspectCertificateMutation())

const form = reactive({ id: '', chain: '', key: '' })
const inspected = ref<InspectedCertificate>()
const problem = ref<string>()

watch(open, () => {
  Object.assign(form, { id: '', chain: '', key: '' })
  inspected.value = undefined
  problem.value = undefined
})
watch(
  () => [form.chain, form.key],
  () => {
    inspected.value = undefined
    problem.value = undefined
  },
)

const idError = computed(() => {
  if (props.certificate || form.id === '') {
    return null
  }
  if (!CERTIFICATE_ID.test(form.id)) {
    return t('certificates.idHint')
  }
  return props.taken.includes(form.id) ? t('certificates.idTaken') : null
})
const busy = computed(() => create.isPending.value || replace.isPending.value)

async function read(event: Event, field: 'chain' | 'key') {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  if (file) {
    form[field] = await file.text()
  }
  input.value = ''
}

function check() {
  inspect.mutate(
    { body: { chain: form.chain, key: form.key || null } },
    {
      onSuccess: (result) => {
        inspected.value = result
      },
      onError: (error) => {
        const failure = toApiFailure(error)
        problem.value =
          failure.kind === 'problem'
            ? (failure.problem.detail ?? failure.problem.title)
            : t('common.changeFailed')
      },
    },
  )
}

function done(message: string) {
  toast.success(message)
  open.value = false
  emit('saved')
}

function submit() {
  const onError = (error: unknown) => notifyFailure(error, t('common.changeFailed'))
  if (props.certificate) {
    replace.mutate(
      {
        path: { id: props.certificate.id },
        body: { chain: form.chain, key: form.key },
        headers: changeHeaders(props.certificate.etag),
      },
      { onSuccess: () => done(t('certificates.replaced')), onError },
    )
    return
  }
  create.mutate(
    {
      body: { source: 'upload', id: form.id.trim(), chain: form.chain, key: form.key },
      headers: plainHeaders(),
    },
    { onSuccess: (created) => done(t('certificates.uploaded', { id: created.id })), onError },
  )
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-xl">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle class="flex items-center gap-2">
            <Upload class="size-5" aria-hidden="true" />
            {{
              certificate
                ? t('certificates.replaceTitle', { id: certificate.id })
                : t('certificates.upload')
            }}
          </SheetTitle>
          <SheetDescription>{{
            certificate ? t('certificates.replaceDetail') : t('certificates.uploadDetail')
          }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-4 px-4">
          <FormField
            v-if="!certificate"
            id="certificate-id"
            :label="t('certificates.id')"
            :hint="idError ?? t('certificates.idHint')"
          >
            <Input
              id="certificate-id"
              v-model="form.id"
              required
              :aria-invalid="idError !== null"
              class="font-mono text-xs"
              placeholder="example.com"
              autocomplete="off"
            />
          </FormField>
          <FormField
            id="certificate-chain"
            :label="t('certificates.chain')"
            :hint="t('certificates.chainHint')"
          >
            <Textarea
              id="certificate-chain"
              v-model="form.chain"
              required
              rows="6"
              spellcheck="false"
              class="font-mono text-xs"
              placeholder="-----BEGIN CERTIFICATE-----"
            />
            <Button variant="outline" size="sm" class="self-start" as-child>
              <label class="cursor-pointer">
                <FileUp data-icon="inline-start" aria-hidden="true" />
                {{ t('certificates.chooseFile') }}
                <input
                  type="file"
                  accept=".pem,.crt,.cer,.txt"
                  class="sr-only"
                  @change="read($event, 'chain')"
                />
              </label>
            </Button>
          </FormField>
          <FormField
            id="certificate-key"
            :label="t('certificates.key')"
            :hint="t('certificates.keyHint')"
          >
            <Textarea
              id="certificate-key"
              v-model="form.key"
              required
              rows="4"
              spellcheck="false"
              autocomplete="off"
              class="font-mono text-xs"
              placeholder="-----BEGIN PRIVATE KEY-----"
            />
            <Button variant="outline" size="sm" class="self-start" as-child>
              <label class="cursor-pointer">
                <FileUp data-icon="inline-start" aria-hidden="true" />
                {{ t('certificates.chooseFile') }}
                <input
                  type="file"
                  accept=".pem,.key,.txt"
                  class="sr-only"
                  @change="read($event, 'key')"
                />
              </label>
            </Button>
          </FormField>
          <Button
            type="button"
            variant="secondary"
            size="sm"
            class="self-start"
            :disabled="!form.chain || inspect.isPending.value"
            @click="check"
          >
            <ScanSearch data-icon="inline-start" aria-hidden="true" />
            {{ t('certificates.check') }}
          </Button>
          <Alert v-if="inspected" role="status">
            <CircleCheck aria-hidden="true" />
            <AlertTitle>{{
              inspected.key_matches ? t('certificates.keyMatches') : t('certificates.chainReadable')
            }}</AlertTitle>
            <AlertDescription>
              <span class="font-mono text-xs">{{ inspected.names.join(', ') }}</span>
              <span class="text-xs"
                >{{ t('certificates.notAfter') }}:
                {{ d(new Date(inspected.not_after), 'datetime') }} ·
                {{ t(`certificates.status.${inspected.status}`) }}</span
              >
            </AlertDescription>
          </Alert>
          <Alert v-else-if="problem" variant="destructive" role="alert">
            <TriangleAlert aria-hidden="true" />
            <AlertDescription>{{ problem }}</AlertDescription>
          </Alert>
        </div>
        <SheetFooter>
          <Button type="submit" :disabled="busy || idError !== null || !form.chain || !form.key">
            <Upload data-icon="inline-start" aria-hidden="true" />
            {{ certificate ? t('certificates.replace') : t('certificates.upload') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
