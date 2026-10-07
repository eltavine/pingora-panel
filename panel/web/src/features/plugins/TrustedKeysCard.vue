<script setup lang="ts">
import { computed, ref } from 'vue'
import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { BadgeCheck, Ellipsis, KeySquare, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import { deletePluginKey, trustPluginKey, type TrustedKeyView } from '@/api/generated'
import { listPluginKeysOptions } from '@/api/generated/@tanstack/vue-query.gen'
import ApiFailureAlert from '@/components/ApiFailureAlert.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
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
import { Textarea } from '@/components/ui/textarea'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { invalidateTagged } from '@/lib/query'

defineProps<{ manages: boolean }>()

const { t, d } = useI18n()
const client = useQueryClient()
const keys = useQuery(listPluginKeysOptions())
const listed = computed(() => keys.data.value ?? [])

const addOpen = ref(false)
const id = ref('')
const publicKey = ref('')
const comment = ref('')
const working = ref(false)
async function add() {
  working.value = true
  try {
    const { data } = await trustPluginKey({
      body: { id: id.value.trim(), public_key: publicKey.value, comment: comment.value.trim() },
      headers: plainHeaders(),
      throwOnError: true,
    })
    toast.success(t('plugins.keys.trusted', { id: data.id }))
    id.value = ''
    publicKey.value = ''
    comment.value = ''
    void invalidateTagged(client, ['plugins'])
  } catch (error) {
    notifyFailure(error, t('plugins.keys.failed'))
  } finally {
    working.value = false
  }
}

const chosen = ref<TrustedKeyView>()
const removeOpen = ref(false)
function askRemove(key: TrustedKeyView) {
  chosen.value = key
  removeOpen.value = true
}
async function remove() {
  const key = chosen.value
  if (!key) {
    return
  }
  try {
    await deletePluginKey({ path: { id: key.id }, headers: plainHeaders(), throwOnError: true })
    toast.success(t('plugins.keys.removed'))
    void invalidateTagged(client, ['plugins'])
  } catch (error) {
    notifyFailure(error, t('plugins.keys.failed'))
  }
}
</script>

<template>
  <Card>
    <CardHeader class="flex flex-row items-start justify-between gap-4">
      <div class="flex flex-col gap-1.5">
        <CardTitle class="flex items-center gap-2">
          <BadgeCheck class="size-4" aria-hidden="true" />{{ t('plugins.keys.title') }}
        </CardTitle>
        <CardDescription>{{ t('plugins.keys.description') }}</CardDescription>
      </div>
      <Button v-if="manages" size="sm" variant="outline" @click="addOpen = true">
        <KeySquare data-icon="inline-start" aria-hidden="true" />{{ t('plugins.keys.add') }}
      </Button>
    </CardHeader>
    <CardContent>
      <ApiFailureAlert
        v-if="keys.isError.value && !keys.data.value"
        :error="keys.error.value"
        retryable
        @retry="keys.refetch()"
      />
      <Skeleton v-else-if="keys.isPending.value" class="h-16 rounded-lg" />
      <ul v-else-if="listed.length" class="divide-y">
        <li v-for="key in listed" :key="key.id" class="flex items-start gap-3 py-2.5">
          <KeySquare class="text-muted-foreground mt-0.5 size-4 shrink-0" aria-hidden="true" />
          <div class="flex min-w-0 flex-1 flex-col gap-0.5">
            <span class="text-sm font-medium">{{ key.id }}</span>
            <span class="text-muted-foreground font-mono text-xs break-all">
              {{ key.key_id }}
            </span>
            <span v-if="key.comment" class="text-muted-foreground text-xs">{{ key.comment }}</span>
            <span class="text-muted-foreground text-xs">
              {{ t('plugins.keys.added') }} {{ d(new Date(key.created_at), 'datetime') }}
            </span>
          </div>
          <DropdownMenu v-if="manages">
            <DropdownMenuTrigger as-child>
              <Button
                variant="ghost"
                size="icon-sm"
                :aria-label="t('plugins.keys.actionsFor', { id: key.id })"
              >
                <Ellipsis aria-hidden="true" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem variant="destructive" @select="askRemove(key)">
                <Trash2 aria-hidden="true" />{{ t('plugins.keys.remove') }}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </li>
      </ul>
      <Empty v-else class="border border-dashed">
        <EmptyHeader>
          <EmptyMedia variant="icon"><KeySquare aria-hidden="true" /></EmptyMedia>
          <EmptyTitle>{{ t('plugins.keys.empty') }}</EmptyTitle>
        </EmptyHeader>
      </Empty>
    </CardContent>
  </Card>

  <ConfirmDialog
    v-model:open="addOpen"
    :icon="KeySquare"
    :title="t('plugins.keys.addTitle')"
    :description="t('plugins.keys.addDetail')"
    :confirm-label="t('plugins.keys.add')"
    :busy="working || !id.trim() || !publicKey.trim()"
    @confirm="add"
  >
    <div class="flex flex-col gap-3">
      <FormField id="plugin-key-id" :label="t('plugins.keys.id')">
        <Input id="plugin-key-id" v-model="id" autocomplete="off" spellcheck="false" />
      </FormField>
      <FormField id="plugin-key-public" :label="t('plugins.keys.publicKey')">
        <Textarea
          id="plugin-key-public"
          v-model="publicKey"
          class="min-h-20 font-mono text-xs"
          spellcheck="false"
        />
      </FormField>
      <FormField id="plugin-key-comment" :label="t('plugins.keys.comment')">
        <Input id="plugin-key-comment" v-model="comment" autocomplete="off" />
      </FormField>
    </div>
  </ConfirmDialog>

  <ConfirmDialog
    v-model:open="removeOpen"
    :icon="Trash2"
    :title="t('plugins.keys.removeTitle', { id: chosen?.id ?? '' })"
    :description="t('plugins.keys.removeDetail')"
    :confirm-label="t('plugins.keys.remove')"
    destructive
    @confirm="remove"
  />
</template>
