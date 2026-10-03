<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Crown, Globe, Plus, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { Domain, SiteView } from '@/api/generated'
import {
  listTlsProfilesOptions,
  removeDomainMutation,
  replaceDomainMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from '@/components/ui/empty'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { changeHeaders, notifyFailure } from '@/lib/configuration'
import DomainAddSheet from './DomainAddSheet.vue'

const INHERIT = '-'

const props = defineProps<{ site: SiteView }>()
const emit = defineEmits<{ saved: [site: SiteView] }>()

const { t } = useI18n()
const profiles = useQuery(listTlsProfilesOptions())
const replace = useMutation(replaceDomainMutation())
const remove = useMutation(removeDomainMutation())
const adding = ref(false)
const removing = ref<Domain | null>(null)
const removeOpen = computed({
  get: () => removing.value !== null,
  set: (open) => {
    if (!open) {
      removing.value = null
    }
  },
})
const domains = computed(() => props.site.domains ?? [])

const onError = (error: unknown) => notifyFailure(error, t('common.changeFailed'))

function update(domain: Domain, patch: Partial<Domain>) {
  replace.mutate(
    {
      path: { id: props.site.id, host: domain.host },
      body: { ...domain, ...patch },
      headers: changeHeaders(props.site.etag),
    },
    { onSuccess: (site) => emit('saved', site), onError },
  )
}

function confirmRemove() {
  const domain = removing.value
  if (!domain) {
    return
  }
  remove.mutate(
    { path: { id: props.site.id, host: domain.host }, headers: changeHeaders(props.site.etag) },
    {
      onSuccess: (site) => {
        toast.success(t('domains.removed'))
        removing.value = null
        emit('saved', site)
      },
      onError,
    },
  )
}

function unicode(domain: Domain) {
  const display = props.site.unicode_hosts?.[domain.host]
  return display && display !== domain.host ? display : null
}
</script>

<template>
  <div class="flex flex-col gap-4">
    <div class="flex justify-end">
      <Button size="sm" @click="adding = true">
        <Plus data-icon="inline-start" aria-hidden="true" />
        {{ t('domains.add') }}
      </Button>
    </div>

    <Empty v-if="domains.length === 0" class="border">
      <EmptyHeader>
        <EmptyMedia variant="icon"><Globe aria-hidden="true" /></EmptyMedia>
        <EmptyTitle>{{ t('domains.emptyTitle') }}</EmptyTitle>
        <EmptyDescription>{{ t('domains.emptyDetail') }}</EmptyDescription>
      </EmptyHeader>
      <EmptyContent>
        <Button size="sm" @click="adding = true">
          <Plus data-icon="inline-start" aria-hidden="true" />
          {{ t('domains.add') }}
        </Button>
      </EmptyContent>
    </Empty>

    <div v-else class="rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{{ t('domains.host') }}</TableHead>
            <TableHead>{{ t('domains.primary') }}</TableHead>
            <TableHead>{{ t('domains.alias') }}</TableHead>
            <TableHead>{{ t('common.enabled') }}</TableHead>
            <TableHead>{{ t('domains.tls') }}</TableHead>
            <TableHead class="w-12"
              ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="domain in domains" :key="domain.host">
            <TableCell class="max-w-72">
              <div class="flex flex-col">
                <span class="truncate font-mono text-xs">{{ domain.host }}</span>
                <span v-if="unicode(domain)" class="text-muted-foreground truncate text-xs">
                  {{ unicode(domain) }}
                </span>
              </div>
            </TableCell>
            <TableCell>
              <Badge v-if="domain.primary" variant="secondary" class="gap-1">
                <Crown class="size-3.5" aria-hidden="true" />
                {{ t('domains.primary') }}
              </Badge>
              <Button
                v-else
                variant="ghost"
                size="xs"
                :disabled="replace.isPending.value"
                @click="update(domain, { primary: true, redirect: false })"
              >
                <Crown data-icon="inline-start" aria-hidden="true" />
                {{ t('domains.makePrimary') }}
              </Button>
            </TableCell>
            <TableCell>
              <Switch
                :model-value="domain.redirect ?? false"
                :disabled="domain.primary || replace.isPending.value"
                :aria-label="t('domains.toggleAlias')"
                @update:model-value="update(domain, { redirect: $event })"
              />
            </TableCell>
            <TableCell>
              <Switch
                :model-value="domain.enabled ?? true"
                :disabled="replace.isPending.value"
                :aria-label="t('common.enabled')"
                @update:model-value="update(domain, { enabled: $event })"
              />
            </TableCell>
            <TableCell>
              <Select
                :model-value="domain.tls_profile_id ?? INHERIT"
                @update:model-value="
                  update(domain, { tls_profile_id: $event === INHERIT ? null : String($event) })
                "
              >
                <SelectTrigger size="sm" class="w-40" :aria-label="t('domains.tls')">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem :value="INHERIT">{{
                    site.tls_profile_id ?? t('state.none')
                  }}</SelectItem>
                  <SelectItem
                    v-for="item in profiles.data.value ?? []"
                    :key="item.id"
                    :value="item.id"
                  >
                    {{ item.id }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </TableCell>
            <TableCell>
              <Button
                variant="ghost"
                size="icon-sm"
                :aria-label="t('common.remove')"
                @click="removing = domain"
              >
                <Trash2 aria-hidden="true" />
              </Button>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <DomainAddSheet v-model:open="adding" :site="site" @saved="emit('saved', $event)" />

    <ConfirmDialog
      v-model:open="removeOpen"
      :icon="Trash2"
      :title="t('domains.confirmRemoveTitle', { host: removing?.host ?? '' })"
      :description="t('domains.confirmRemoveDetail')"
      :confirm-label="t('common.remove')"
      destructive
      :busy="remove.isPending.value"
      @confirm="confirmRemove"
    />
  </div>
</template>
