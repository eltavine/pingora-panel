<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMutation, useQueryClient } from '@tanstack/vue-query'
import { Ellipsis, OctagonX, Play, RotateCw, Square, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { ContainerActionName, ContainerView } from '@/api/generated'
import {
  actOnContainerMutation,
  listContainersQueryKey,
  removeContainerMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { notifyFailure, plainHeaders } from '@/lib/configuration'
import { stoppable } from './presentation'

const props = defineProps<{ engine: string; container: ContainerView }>()

const { t } = useI18n()
const queryClient = useQueryClient()
const act = useMutation(actOnContainerMutation())
const remove = useMutation(removeContainerMutation())
const busy = computed(() => act.isPending.value || remove.isPending.value)
const name = computed(() => props.container.names[0] ?? props.container.id.slice(0, 12))
const running = computed(() => stoppable(props.container.state))

/** An action that stops what runs in the container, awaiting confirmation. */
const confirming = ref<Exclude<ContainerActionName, 'start'> | null>(null)
const confirmOpen = computed({
  get: () => confirming.value !== null,
  set: (open: boolean) => {
    if (!open) {
      confirming.value = null
    }
  },
})
const removing = ref(false)
const force = ref(false)
const volumes = ref(false)

const icons = { stop: Square, restart: RotateCw, kill: OctagonX } as const

function done(message: string) {
  toast.success(message)
  void queryClient.invalidateQueries({
    queryKey: listContainersQueryKey({ path: { engine: props.engine } }),
  })
}

function run(action: ContainerActionName) {
  act.mutate(
    {
      path: { engine: props.engine, container: props.container.id, action },
      headers: plainHeaders(),
    },
    {
      onSuccess: () => done(t(`containers.actions.done.${action}`, { name: name.value })),
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

function confirmRemove() {
  remove.mutate(
    {
      path: { engine: props.engine, container: props.container.id },
      query: { force: force.value, volumes: volumes.value },
      headers: plainHeaders(),
    },
    {
      onSuccess: () => done(t('containers.actions.done.remove', { name: name.value })),
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}

function openRemove() {
  force.value = false
  volumes.value = false
  removing.value = true
}
</script>

<template>
  <DropdownMenu>
    <DropdownMenuTrigger as-child>
      <Button
        variant="ghost"
        size="icon-sm"
        :disabled="busy"
        :aria-label="t('containers.actions.menu', { name })"
      >
        <Ellipsis aria-hidden="true" />
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align="end">
      <DropdownMenuItem v-if="!running" @select="run('start')">
        <Play aria-hidden="true" />{{ t('containers.actions.start') }}
      </DropdownMenuItem>
      <template v-else>
        <DropdownMenuItem @select="confirming = 'restart'">
          <RotateCw aria-hidden="true" />{{ t('containers.actions.restart') }}
        </DropdownMenuItem>
        <DropdownMenuItem @select="confirming = 'stop'">
          <Square aria-hidden="true" />{{ t('containers.actions.stop') }}
        </DropdownMenuItem>
        <DropdownMenuItem variant="destructive" @select="confirming = 'kill'">
          <OctagonX aria-hidden="true" />{{ t('containers.actions.kill') }}
        </DropdownMenuItem>
      </template>
      <DropdownMenuSeparator />
      <DropdownMenuItem variant="destructive" @select="openRemove">
        <Trash2 aria-hidden="true" />{{ t('containers.actions.remove') }}
      </DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>

  <ConfirmDialog
    v-if="confirming"
    v-model:open="confirmOpen"
    :icon="icons[confirming]"
    :title="t(`containers.actions.confirm.${confirming}.title`, { name })"
    :description="t(`containers.actions.confirm.${confirming}.detail`)"
    :confirm-label="t(`containers.actions.${confirming}`)"
    :destructive="confirming === 'kill'"
    :busy="busy"
    @confirm="confirming && run(confirming)"
  />
  <ConfirmDialog
    v-model:open="removing"
    :icon="Trash2"
    :title="t('containers.actions.confirm.remove.title', { name })"
    :description="t('containers.actions.confirm.remove.detail')"
    :confirm-label="t('containers.actions.remove')"
    destructive
    :busy="busy"
    @confirm="confirmRemove"
  >
    <div class="flex flex-col gap-2">
      <label v-if="running" class="flex items-start gap-2 text-sm">
        <Checkbox v-model="force" class="mt-0.5" />
        <span>{{ t('containers.actions.force') }}</span>
      </label>
      <label class="flex items-start gap-2 text-sm">
        <Checkbox v-model="volumes" class="mt-0.5" />
        <span>{{ t('containers.actions.volumes') }}</span>
      </label>
    </div>
  </ConfirmDialog>
</template>
