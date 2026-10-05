<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { CircleAlert, CircleCheck, CircleMinus, FlaskConical } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { testRoute, type RouteTestResult, type RouteView, type SiteView } from '@/api/generated'
import FormField from '@/components/FormField.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
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
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Spinner } from '@/components/ui/spinner'
import { Textarea } from '@/components/ui/textarea'
import { notifyFailure } from '@/lib/configuration'
import { headerLines } from './conditions'

const METHODS = ['GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS'] as const

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ site: SiteView; routes: readonly RouteView[] }>()

const { t } = useI18n()
const form = reactive({ method: 'GET', host: '', target: '/', headers: '', client: '' })
const result = ref<RouteTestResult>()
const testing = ref(false)

watch(open, (isOpen) => {
  if (isOpen && !form.host) {
    form.host = props.site.domains?.find((domain) => !domain.host.startsWith('*.'))?.host ?? ''
  }
})

const names = computed(
  () => new Map(props.routes.map((route) => [route.id, route.name ?? route.match.path])),
)

/** A route as the tester names it: the site's own action, a name, or a path. */
function routeName(id: string | null | undefined): string {
  if (!id) {
    return ''
  }
  return id.endsWith('-site') ? t('routes.tester.siteAction') : (names.value.get(id) ?? id)
}

async function run() {
  testing.value = true
  try {
    const { data } = await testRoute({
      body: {
        method: form.method,
        host: form.host.trim(),
        target: form.target.trim() || '/',
        headers: headerLines(form.headers),
        client: form.client.trim() || null,
      },
      throwOnError: true,
    })
    result.value = data
  } catch (error) {
    notifyFailure(error, t('routes.tester.failed'))
  } finally {
    testing.value = false
  }
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-xl">
      <form class="flex flex-col gap-6" @submit.prevent="run">
        <SheetHeader>
          <SheetTitle class="flex items-center gap-2">
            <FlaskConical class="size-5" aria-hidden="true" />
            {{ t('routes.tester.title') }}
          </SheetTitle>
          <SheetDescription>{{ t('routes.tester.description') }}</SheetDescription>
        </SheetHeader>

        <div class="flex flex-col gap-4 px-4">
          <div class="grid gap-4 sm:grid-cols-[8rem_1fr]">
            <FormField id="tester-method" :label="t('routes.tester.method')">
              <Select v-model="form.method">
                <SelectTrigger id="tester-method" class="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="method in METHODS" :key="method" :value="method">
                    {{ method }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </FormField>
            <FormField id="tester-host" :label="t('routes.tester.host')">
              <Input
                id="tester-host"
                v-model="form.host"
                required
                class="font-mono text-xs"
                autocomplete="off"
                list="tester-hosts"
              />
              <datalist id="tester-hosts">
                <option
                  v-for="domain in site.domains ?? []"
                  :key="domain.host"
                  :value="domain.host"
                />
              </datalist>
            </FormField>
          </div>
          <FormField id="tester-target" :label="t('routes.tester.target')">
            <Input
              id="tester-target"
              v-model="form.target"
              class="font-mono text-xs"
              autocomplete="off"
              placeholder="/api/items?tag=new"
            />
          </FormField>
          <FormField
            id="tester-headers"
            :label="t('routes.tester.headers')"
            :hint="t('routes.tester.headersHint')"
          >
            <Textarea
              id="tester-headers"
              v-model="form.headers"
              rows="3"
              class="font-mono text-xs"
              placeholder="X-Env: staging"
            />
          </FormField>
          <FormField id="tester-client" :label="t('routes.tester.client')">
            <Input
              id="tester-client"
              v-model="form.client"
              class="font-mono text-xs"
              autocomplete="off"
              placeholder="203.0.113.9"
            />
          </FormField>
          <Button type="submit" class="self-start" :disabled="testing || !form.host.trim()">
            <Spinner v-if="testing" data-icon="inline-start" />
            <FlaskConical v-else data-icon="inline-start" aria-hidden="true" />
            {{ t('routes.tester.run') }}
          </Button>
        </div>

        <section v-if="result" class="flex flex-col gap-3 px-4 pb-4" aria-live="polite">
          <div
            class="flex items-start gap-2 rounded-lg border p-3 text-sm"
            :class="result.outcome === 'routed' ? 'bg-muted/40' : 'border-destructive/50'"
          >
            <CircleCheck
              v-if="result.outcome === 'routed'"
              class="mt-0.5 size-4 shrink-0"
              aria-hidden="true"
            />
            <CircleAlert
              v-else
              class="text-destructive mt-0.5 size-4 shrink-0"
              aria-hidden="true"
            />
            <div class="flex min-w-0 flex-col gap-0.5">
              <span class="font-medium">
                {{
                  t(`routes.tester.outcomes.${result.outcome}`, {
                    route: routeName(result.route_id),
                    host: result.host,
                  })
                }}
              </span>
              <span class="text-muted-foreground font-mono text-xs break-all">
                {{
                  t('routes.tester.draft', {
                    version: result.draft_version,
                    host: result.host,
                    path: result.path,
                  })
                }}
              </span>
            </div>
          </div>
          <template v-if="result.routes.length">
            <h3 class="text-sm font-medium">{{ t('routes.tester.tried') }}</h3>
            <ol class="flex flex-col gap-2">
              <li
                v-for="trial in result.routes"
                :key="trial.route_id"
                class="flex items-start gap-2 text-sm"
              >
                <CircleCheck
                  v-if="trial.matched"
                  class="mt-0.5 size-4 shrink-0"
                  aria-hidden="true"
                />
                <CircleMinus
                  v-else
                  class="text-muted-foreground mt-0.5 size-4 shrink-0"
                  aria-hidden="true"
                />
                <div class="flex min-w-0 flex-col gap-0.5">
                  <span class="font-medium">
                    {{ routeName(trial.route_id) }}
                    <span class="sr-only">
                      {{ trial.matched ? t('routes.tester.takes') : t('routes.tester.skipped') }}
                    </span>
                  </span>
                  <span
                    v-if="trial.reason"
                    class="text-muted-foreground font-mono text-xs break-words"
                  >
                    {{ trial.reason }}
                  </span>
                </div>
              </li>
            </ol>
          </template>
        </section>
      </form>
    </SheetContent>
  </Sheet>
</template>
