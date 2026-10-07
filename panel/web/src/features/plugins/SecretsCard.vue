<script setup lang="ts">
import { computed, ref } from 'vue'
import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { Ellipsis, LockKeyhole, Trash2, Vault } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import { deletePluginSecret, putPluginSecret, type SecretView } from '@/api/generated'
import { listPluginSecretsOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import CopyValue from '@/components/CopyValue.vue'
import FormField from '@/components/FormField.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Empty, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { invalidateTagged } from '@/lib/query'

defineProps<{ manages: boolean }>()

const { t, d } = useI18n()
const client = useQueryClient()
const secrets = useQuery(listPluginSecretsOptions())
const listed = computed(() => secrets.data.value ?? [])

const setOpen = ref(false)
const name = ref('')
const value = ref('')
const working = ref(false)
async function keep() {
  working.value = true
  try {
    const { data } = await putPluginSecret({
      path: { name: name.value.trim() },
      body: { value: value.value },
      headers: plainHeaders(),
      throwOnError: true,
    })
    toast.success(t('plugins.secrets.saved', { name: data.name }))
    name.value = ''
    void invalidateTagged(client, ['plugins'])
  } catch (error) {
    notifyFailure(error, t('plugins.secrets.failed'))
  } finally {
    value.value = ''
    working.value = false
  }
}

const chosen = ref<SecretView>()
const removeOpen = ref(false)
function askRemove(secret: SecretView) {
  chosen.value = secret
  removeOpen.value = true
}
async function remove() {
  const secret = chosen.value
  if (!secret) {
    return
  }
  try {
    await deletePluginSecret({
      path: { name: secret.name },
      headers: plainHeaders(),
      throwOnError: true,
    })
    toast.success(t('plugins.secrets.removed'))
    void invalidateTagged(client, ['plugins'])
  } catch (error) {
    notifyFailure(error, t('plugins.secrets.failed'))
  }
}
</script>

<template>
  <Card>
    <CardHeader class="flex flex-row items-start justify-between gap-4">
      <div class="flex flex-col gap-1.5">
        <CardTitle class="flex items-center gap-2">
          <Vault class="size-4" aria-hidden="true" />{{ t('plugins.secrets.title') }}
        </CardTitle>
        <CardDescription>{{ t('plugins.secrets.description') }}</CardDescription>
      </div>
      <Button v-if="manages" size="sm" variant="outline" @click="setOpen = true">
        <LockKeyhole data-icon="inline-start" aria-hidden="true" />{{ t('plugins.secrets.set') }}
      </Button>
    </CardHeader>
    <CardContent>
      <ApiFailureAlert
        v-if="secrets.isError.value && !secrets.data.value"
        :error="secrets.error.value"
        retryable
        @retry="secrets.refetch()"
      />
      <Skeleton v-else-if="secrets.isPending.value" class="h-16 rounded-lg" />
      <ul v-else-if="listed.length" class="divide-y">
        <li v-for="secret in listed" :key="secret.name" class="flex items-center gap-3 py-2.5">
          <LockKeyhole class="text-muted-foreground size-4 shrink-0" aria-hidden="true" />
          <div class="flex min-w-0 flex-1 flex-col gap-0.5">
            <CopyValue :value="`vault:${secret.name}`" class="font-mono text-sm" />
            <span class="text-muted-foreground text-xs">
              {{ t('plugins.secrets.updated') }} {{ d(new Date(secret.updated_at), 'datetime') }}
            </span>
          </div>
          <DropdownMenu v-if="manages">
            <DropdownMenuTrigger as-child>
              <Button
                variant="ghost"
                size="icon-sm"
                :aria-label="t('plugins.secrets.actionsFor', { name: secret.name })"
              >
                <Ellipsis aria-hidden="true" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem variant="destructive" @select="askRemove(secret)">
                <Trash2 aria-hidden="true" />{{ t('plugins.secrets.remove') }}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </li>
      </ul>
      <Empty v-else class="border border-dashed">
        <EmptyHeader>
          <EmptyMedia variant="icon"><Vault aria-hidden="true" /></EmptyMedia>
          <EmptyTitle>{{ t('plugins.secrets.empty') }}</EmptyTitle>
        </EmptyHeader>
      </Empty>
    </CardContent>
  </Card>

  <ConfirmDialog
    v-model:open="setOpen"
    :icon="LockKeyhole"
    :title="t('plugins.secrets.setTitle')"
    :description="t('plugins.secrets.setDetail')"
    :confirm-label="t('plugins.secrets.set')"
    :busy="working || !name.trim() || !value"
    @confirm="keep"
  >
    <div class="flex flex-col gap-3">
      <FormField id="plugin-secret-name" :label="t('plugins.secrets.name')">
        <Input
          id="plugin-secret-name"
          v-model="name"
          class="font-mono"
          autocomplete="off"
          spellcheck="false"
        />
      </FormField>
      <FormField id="plugin-secret-value" :label="t('plugins.secrets.value')">
        <Input
          id="plugin-secret-value"
          v-model="value"
          type="password"
          autocomplete="new-password"
        />
      </FormField>
    </div>
  </ConfirmDialog>

  <ConfirmDialog
    v-model:open="removeOpen"
    :icon="Trash2"
    :title="t('plugins.secrets.removeTitle', { name: chosen?.name ?? '' })"
    :description="t('plugins.secrets.removeDetail')"
    :confirm-label="t('plugins.secrets.remove')"
    destructive
    @confirm="remove"
  />
</template>
