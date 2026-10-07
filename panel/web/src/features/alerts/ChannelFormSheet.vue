<script setup lang="ts">
import { reactive, watch } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { KeyRound, Plus, Puzzle, Webhook } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { AlertChannelSecretView, AlertChannelView, NewAlertChannelKind } from '@/api/generated'
import ChoiceCards, { type Choice } from '@/components/ChoiceCards.vue'
import {
  createAlertChannelMutation,
  rotateAlertChannelMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
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
import { changeHeaders, notifyFailure, plainHeaders } from '@/lib/configuration'

const open = defineModel<boolean>('open', { required: true })
/** Without a channel the sheet creates one; with one it rotates it. */
const props = defineProps<{ channel?: AlertChannelView }>()
const emit = defineEmits<{ secret: [secret: AlertChannelSecretView] }>()

const { t } = useI18n()
const create = useMutation(createAlertChannelMutation())
const rotate = useMutation(rotateAlertChannelMutation())
type Kind = Extract<NewAlertChannelKind, 'webhook' | 'plugin'>
const form = reactive({ id: '', kind: 'webhook' as Kind, url: '', plugin: '', pluginChannel: '' })
const kinds: readonly Choice<Kind>[] = [
  { value: 'webhook', label: t('alerts.kinds.webhook'), icon: Webhook },
  { value: 'plugin', label: t('alerts.kinds.plugin'), icon: Puzzle },
]
watch(open, (isOpen) => {
  if (isOpen) {
    form.id = ''
    form.kind = 'webhook'
    form.url = ''
    form.plugin = ''
    form.pluginChannel = ''
  }
})

function done(secret: AlertChannelSecretView) {
  form.url = ''
  open.value = false
  emit('secret', secret)
}

function submit() {
  const onError = (error: unknown) => notifyFailure(error, t('common.changeFailed'))
  if (props.channel) {
    rotate.mutate(
      {
        path: { id: props.channel.id },
        body: { url: form.url.trim() || null },
        headers: changeHeaders(props.channel.etag),
      },
      { onSuccess: done, onError },
    )
  } else {
    const body =
      form.kind === 'plugin'
        ? {
            id: form.id.trim(),
            kind: form.kind,
            plugin: form.plugin.trim(),
            plugin_channel: form.pluginChannel.trim() || null,
          }
        : { id: form.id.trim(), kind: form.kind, url: form.url.trim() }
    create.mutate({ body, headers: plainHeaders() }, { onSuccess: done, onError })
  }
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>
            {{ channel ? t('alerts.rotateTitle', { id: channel.id }) : t('alerts.newChannel') }}
          </SheetTitle>
          <SheetDescription>
            {{ channel ? t('alerts.rotateDescription') : t('alerts.channelDescription') }}
          </SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-4 px-4">
          <FormField v-if="!channel" id="alert-channel-id" :label="t('alerts.channelId')">
            <Input
              id="alert-channel-id"
              v-model="form.id"
              required
              maxlength="64"
              pattern="[A-Za-z0-9._\-]+"
              class="font-mono"
              autocomplete="off"
            />
          </FormField>
          <ChoiceCards
            v-if="!channel"
            v-model="form.kind"
            :label="t('alerts.columns.kind')"
            :choices="kinds"
          />
          <template v-if="!channel && form.kind === 'plugin'">
            <FormField
              id="alert-channel-plugin"
              :label="t('alerts.plugin')"
              :hint="t('alerts.pluginHint')"
            >
              <Input
                id="alert-channel-plugin"
                v-model="form.plugin"
                required
                maxlength="64"
                pattern="[a-z0-9][a-z0-9\-]*"
                class="font-mono"
                autocomplete="off"
                spellcheck="false"
              />
            </FormField>
            <FormField
              id="alert-channel-plugin-channel"
              :label="t('alerts.pluginChannel')"
              :hint="t('alerts.pluginChannelHint')"
            >
              <Input
                id="alert-channel-plugin-channel"
                v-model="form.pluginChannel"
                maxlength="256"
                autocomplete="off"
              />
            </FormField>
          </template>
          <FormField
            v-else
            id="alert-channel-url"
            :label="channel ? t('alerts.newUrl') : t('alerts.url')"
            :hint="t('alerts.urlHint')"
          >
            <Input
              id="alert-channel-url"
              v-model="form.url"
              type="url"
              :required="!channel"
              class="font-mono text-xs"
              autocomplete="off"
              spellcheck="false"
              placeholder="https://hooks.example/alerts"
              aria-describedby="alert-channel-url-hint"
            />
          </FormField>
        </div>
        <SheetFooter>
          <Button type="submit" :disabled="create.isPending.value || rotate.isPending.value">
            <KeyRound v-if="channel" data-icon="inline-start" aria-hidden="true" />
            <Plus v-else data-icon="inline-start" aria-hidden="true" />
            {{ channel ? t('alerts.rotate') : t('common.create') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
