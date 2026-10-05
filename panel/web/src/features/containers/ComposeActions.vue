<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQueryClient } from '@tanstack/vue-query'
import { ArrowUpFromLine, Ellipsis, PowerOff, RotateCw, ShieldCheck } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { ComposeActionName, ComposeChangeView, ComposeProjectView } from '@/api/generated'
import {
  actOnProjectMutation,
  listContainersQueryKey,
  listNetworksQueryKey,
  listProjectsQueryKey,
} from '@/api/generated/@tanstack/vue-query.gen'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { notifyFailure, plainHeaders } from '@/lib/configuration'

const props = defineProps<{ engine: string; project: ComposeProjectView }>()

const { t } = useI18n()
const queryClient = useQueryClient()
const act = useMutation(actOnProjectMutation())
const name = computed(() => props.project.name)

/** An action that stops what runs in the project, awaiting confirmation. */
const confirming = ref<Exclude<ComposeActionName, 'up'> | null>(null)
const confirmOpen = computed({
  get: () => confirming.value !== null,
  set: (open: boolean) => {
    if (!open) {
      confirming.value = null
    }
  },
})
const icons = { down: PowerOff, restart: RotateCw } as const

function done(action: ComposeActionName, change: ComposeChangeView) {
  if (change.failures.length) {
    toast.warning(
      t(
        'containers.projects.refused',
        { name: name.value, count: change.failures.length },
        change.failures.length,
      ),
      {
        description: change.failures
          .map((failure) => `${failure.name}: ${failure.error.message}`)
          .join('\n'),
      },
    )
  } else {
    toast.success(t(`containers.projects.done.${action}`, { name: name.value }))
  }
  const path = { path: { engine: props.engine } }
  for (const queryKey of [
    listProjectsQueryKey(path),
    listContainersQueryKey(path),
    listNetworksQueryKey(path),
  ]) {
    void queryClient.invalidateQueries({ queryKey })
  }
}

function run(action: ComposeActionName) {
  act.mutate(
    {
      path: { engine: props.engine, project: name.value, action },
      headers: plainHeaders(),
    },
    {
      onSuccess: (change) => done(action, change),
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <DropdownMenu>
    <DropdownMenuTrigger as-child>
      <Button
        variant="ghost"
        size="icon-sm"
        :disabled="act.isPending.value"
        :aria-label="t('containers.actions.menu', { name })"
      >
        <Ellipsis aria-hidden="true" />
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align="end" class="w-56">
      <DropdownMenuItem @select="run('up')">
        <ArrowUpFromLine aria-hidden="true" />{{ t('containers.projects.up') }}
      </DropdownMenuItem>
      <template v-if="project.installation">
        <DropdownMenuSeparator />
        <DropdownMenuLabel class="text-muted-foreground flex gap-2 text-xs font-normal">
          <ShieldCheck class="size-4 shrink-0" aria-hidden="true" />
          {{ t('containers.projects.onlyUp') }}
        </DropdownMenuLabel>
      </template>
      <template v-else>
        <DropdownMenuItem @select="confirming = 'restart'">
          <RotateCw aria-hidden="true" />{{ t('containers.projects.restart') }}
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem variant="destructive" @select="confirming = 'down'">
          <PowerOff aria-hidden="true" />{{ t('containers.projects.down') }}
        </DropdownMenuItem>
      </template>
    </DropdownMenuContent>
  </DropdownMenu>

  <ConfirmDialog
    v-if="confirming"
    v-model:open="confirmOpen"
    :icon="icons[confirming]"
    :title="t(`containers.projects.confirm.${confirming}.title`, { name })"
    :description="t(`containers.projects.confirm.${confirming}.detail`)"
    :confirm-label="t(`containers.projects.${confirming}`)"
    :destructive="confirming === 'down'"
    :busy="act.isPending.value"
    @confirm="confirming && run(confirming)"
  />
</template>
