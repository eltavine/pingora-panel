<script setup lang="ts">
import { computed, ref } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { CircleAlert, Fingerprint, LogIn } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { login } from '@/api/generated'
import { signInOptionsOptions } from '@/api/generated/@tanstack/vue-query.gen'
import FormField from '@/components/FormField.vue'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Separator } from '@/components/ui/separator'
import { Spinner } from '@/components/ui/spinner'
import { adoptSession } from '@/lib/session'
import AuthCard from './AuthCard.vue'
import { signInProblem } from './failures'
import PasswordInput from './PasswordInput.vue'
import { returnPath } from './presentation'
import { startUrl } from './providers'

const { t, te } = useI18n()
const route = useRoute()
const router = useRouter()
const username = ref('')
const password = ref('')
const busy = ref(false)
const problem = ref<string>()
const providers = useQuery({ ...signInOptionsOptions(), retry: false })
const next = computed(() => returnPath(route.query.next))
/** Why signing in through a provider failed, in the console's own words. */
const providerProblem = computed(() => {
  const code = route.query.sign_in_error
  if (typeof code !== 'string') {
    return undefined
  }
  const key = `auth.providerErrors.${code}`
  return te(key) ? t(key) : t('auth.providerErrors.default')
})

async function submit() {
  busy.value = true
  problem.value = undefined
  try {
    const { data } = await login({
      body: { username: username.value, password: password.value, transport: 'cookie' },
      throwOnError: true,
    })
    adoptSession(data)
    await router.replace(next.value)
  } catch (error) {
    problem.value = signInProblem(error, t)
    password.value = ''
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <AuthCard :title="t('auth.title')" :description="t('auth.subtitle')">
    <form class="flex flex-col gap-4" @submit.prevent="submit">
      <Alert v-if="problem ?? providerProblem" variant="destructive" role="alert">
        <CircleAlert aria-hidden="true" />
        <AlertDescription>{{ problem ?? providerProblem }}</AlertDescription>
      </Alert>
      <FormField id="login-username" :label="t('auth.username')">
        <Input
          id="login-username"
          v-model="username"
          autocomplete="username"
          autocapitalize="none"
          spellcheck="false"
          required
          autofocus
        />
      </FormField>
      <FormField id="login-password" :label="t('auth.password')">
        <PasswordInput id="login-password" v-model="password" autocomplete="current-password" />
      </FormField>
      <Button type="submit" :disabled="busy || !username || !password">
        <Spinner v-if="busy" data-icon="inline-start" />
        <LogIn v-else data-icon="inline-start" aria-hidden="true" />
        {{ t('auth.submit') }}
      </Button>
      <template v-if="providers.data.value?.length">
        <div class="text-muted-foreground flex items-center gap-3 text-xs">
          <Separator class="flex-1" />
          {{ t('auth.or') }}
          <Separator class="flex-1" />
        </div>
        <Button
          v-for="provider in providers.data.value"
          :key="provider.id"
          variant="outline"
          as-child
        >
          <a :href="startUrl(provider.id, next)">
            <Fingerprint data-icon="inline-start" aria-hidden="true" />
            {{ t('auth.continueWith', { name: provider.display_name }) }}
          </a>
        </Button>
      </template>
    </form>
  </AuthCard>
</template>
