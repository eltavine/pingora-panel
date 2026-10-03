<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { Save } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { UpstreamNode, UpstreamView } from '@/api/generated'
import { addNodeMutation, replaceNodeMutation } from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import SwitchField from '@/components/SwitchField.vue'
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
import { changeHeaders, notifyFailure, plainHeaders } from '@/lib/configuration'
import { nodeForm, nodeInput, type NodeForm } from './forms'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ upstream: UpstreamView; node?: UpstreamNode }>()
const emit = defineEmits<{ saved: [upstream: UpstreamView] }>()

const { t } = useI18n()
const add = useMutation(addNodeMutation())
const replace = useMutation(replaceNodeMutation())
const busy = computed(() => add.isPending.value || replace.isPending.value)

const form = reactive<NodeForm>(nodeForm())
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, nodeForm(props.node))
  }
})

function saved(upstream: UpstreamView) {
  toast.success(props.node ? t('common.saved') : t('upstreams.node.added'))
  open.value = false
  emit('saved', upstream)
}

function submit() {
  const onError = (error: unknown) => notifyFailure(error, t('common.changeFailed'))
  if (props.node) {
    replace.mutate(
      {
        path: { id: props.upstream.id, node: props.node.id },
        body: nodeInput(form, props.node.id),
        headers: changeHeaders(props.upstream.etag),
      },
      { onSuccess: saved, onError },
    )
  } else {
    add.mutate(
      { path: { id: props.upstream.id }, body: nodeInput(form), headers: plainHeaders() },
      { onSuccess: saved, onError },
    )
  }
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>{{ node ? t('upstreams.node.edit') : t('upstreams.node.add') }}</SheetTitle>
          <SheetDescription>{{ upstream.name }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-4 px-4">
          <div class="grid gap-4 sm:grid-cols-[1fr_7rem]">
            <FormField id="node-host" :label="t('upstreams.node.host')">
              <Input
                id="node-host"
                v-model="form.host"
                required
                class="font-mono text-xs"
                autocomplete="off"
              />
            </FormField>
            <FormField id="node-port" :label="t('upstreams.node.port')">
              <Input
                id="node-port"
                v-model.number="form.port"
                type="number"
                min="1"
                max="65535"
                required
              />
            </FormField>
          </div>
          <FormField id="node-weight" :label="t('upstreams.node.weight')">
            <Input
              id="node-weight"
              v-model.number="form.weight"
              type="number"
              min="1"
              max="65535"
            />
          </FormField>
          <SwitchField id="node-enabled" v-model="form.enabled" :label="t('common.enabled')" />
          <SwitchField id="node-backup" v-model="form.backup" :label="t('upstreams.node.backup')" />
          <SwitchField id="node-tls" v-model="form.tls" :label="t('upstreams.node.tls')" />
          <FormField v-if="form.tls" id="node-sni" :label="t('upstreams.node.sni')">
            <Input id="node-sni" v-model="form.sni" autocomplete="off" />
          </FormField>
          <FormField
            id="node-unix"
            :label="t('upstreams.node.unixSocket')"
            :hint="t('upstreams.node.unixSocketHint')"
          >
            <Input
              id="node-unix"
              v-model="form.unixSocket"
              class="font-mono text-xs"
              placeholder="/run/app.sock"
              autocomplete="off"
            />
          </FormField>
          <FormField id="node-note" :label="t('upstreams.node.note')">
            <Textarea id="node-note" v-model="form.note" rows="2" />
          </FormField>
        </div>
        <SheetFooter>
          <Button type="submit" :disabled="busy">
            <Save data-icon="inline-start" aria-hidden="true" />
            {{ node ? t('common.save') : t('common.add') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
