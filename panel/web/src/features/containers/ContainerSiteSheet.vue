<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useMutation, useQueryClient } from '@tanstack/vue-query'
import { Globe, Info, TriangleAlert } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { toast } from 'vue-sonner'
import type { ContainerEndpointView, ContainerView } from '@/api/generated'
import {
  createContainerSiteMutation,
  siteLinksQueryKey,
} from '@/api/generated/@tanstack/vue-query.gen'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
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
import { Textarea } from '@/components/ui/textarea'
import { notifyFailure, plainHeaders, useRefreshConfiguration } from '@/lib/configuration'
import { defaultEndpoint, endpointAddress } from '@/lib/containerSites'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ engine: string; container: ContainerView }>()

const { t } = useI18n()
const router = useRouter()
const queryClient = useQueryClient()
const refreshConfiguration = useRefreshConfiguration()
const create = useMutation(createContainerSiteMutation())

const label = computed(() => props.container.names[0] ?? props.container.id.slice(0, 12))
const name = ref('')
const hosts = ref('')
const chosen = ref('')

watch(
  open,
  (opened) => {
    if (opened) {
      name.value = props.container.declared_site?.name ?? label.value
      hosts.value = (props.container.declared_site?.domains ?? []).join('\n')
      const endpoint = defaultEndpoint(props.container)
      chosen.value = endpoint ? endpointAddress(endpoint) : ''
    }
  },
  { immediate: true },
)

const endpoint = computed(() =>
  props.container.endpoints.find((candidate) => endpointAddress(candidate) === chosen.value),
)
const domains = computed(() =>
  hosts.value
    .split(/[\s,]+/)
    .map((host) => host.trim())
    .filter(Boolean),
)

function describe(candidate: ContainerEndpointView): string {
  return candidate.route === 'published'
    ? t('containers.site.published', { port: candidate.container_port })
    : t('containers.site.network', {
        port: candidate.container_port,
        network: candidate.network ?? '',
      })
}

function submit() {
  create.mutate(
    {
      path: { engine: props.engine, container: props.container.id },
      body: {
        name: name.value.trim() || undefined,
        domains: domains.value,
        endpoint: endpoint.value
          ? { host: endpoint.value.host, port: endpoint.value.port }
          : undefined,
      },
      headers: plainHeaders(),
    },
    {
      onSuccess: (created) => {
        toast.success(t('containers.site.added', { site: created.site }), {
          description: t('containers.site.apply'),
          action: {
            label: t('containers.site.open'),
            onClick: () => void router.push(`/sites/${created.site_id}`),
          },
        })
        void queryClient.invalidateQueries({
          queryKey: siteLinksQueryKey({ path: { engine: props.engine } }),
        })
        void refreshConfiguration()
        open.value = false
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
          <SheetTitle class="flex items-center gap-2 break-all">
            <Globe class="size-5 shrink-0" aria-hidden="true" />
            {{ t('containers.site.title', { name: label }) }}
          </SheetTitle>
          <SheetDescription>{{ t('containers.site.description') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-4 px-4">
          <div class="flex flex-col gap-2">
            <Label for="container-site-name">{{ t('containers.site.name') }}</Label>
            <Input id="container-site-name" v-model="name" autocomplete="off" required />
          </div>
          <div class="flex flex-col gap-2">
            <Label for="container-site-hosts">{{ t('containers.site.hosts') }}</Label>
            <Textarea
              id="container-site-hosts"
              v-model="hosts"
              class="font-mono"
              rows="3"
              placeholder="shop.example"
              spellcheck="false"
              required
              aria-describedby="container-site-hosts-hint"
            />
            <p id="container-site-hosts-hint" class="text-muted-foreground text-xs">
              {{ t('containers.site.hostsHint') }}
            </p>
          </div>
          <div class="flex flex-col gap-2">
            <Label for="container-site-endpoint">{{ t('containers.site.endpoint') }}</Label>
            <Select v-model="chosen">
              <SelectTrigger id="container-site-endpoint" class="w-full font-mono">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem
                  v-for="candidate in container.endpoints"
                  :key="endpointAddress(candidate)"
                  :value="endpointAddress(candidate)"
                >
                  <span class="font-mono">{{ endpointAddress(candidate) }}</span>
                  <span class="text-muted-foreground text-xs">{{
                    ` · ${describe(candidate)}`
                  }}</span>
                </SelectItem>
              </SelectContent>
            </Select>
            <p
              v-if="endpoint?.route === 'network'"
              class="text-muted-foreground flex items-start gap-2 text-xs"
            >
              <TriangleAlert class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
              {{ t('containers.site.networkWarning') }}
            </p>
          </div>
          <p class="text-muted-foreground flex items-start gap-2 text-xs">
            <Info class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
            {{ t('containers.site.draft') }}
          </p>
        </div>
        <SheetFooter>
          <Button
            type="submit"
            :disabled="create.isPending.value || !endpoint || domains.length === 0"
          >
            <Globe data-icon="inline-start" aria-hidden="true" />
            {{ t('containers.site.submit') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
