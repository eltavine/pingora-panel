<script setup lang="ts">
import { ref } from 'vue'
import { CircleAlert, LogIn } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { login } from '@/api/generated'
import FormField from '@/components/FormField.vue'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Spinner } from '@/components/ui/spinner'
import { adoptSession } from '@/lib/session'
import AuthCard from './AuthCard.vue'
import { signInProblem } from './failures'
import PasswordInput from './PasswordInput.vue'
import { returnPath } from './presentation'

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
const username = ref('')
const password = ref('')
const busy = ref(false)
const problem = ref<string>()

async function submit() {
  busy.value = true
  problem.value = undefined
  try {
    const { data } = await login({
      body: { username: username.value, password: password.value, transport: 'cookie' },
      throwOnError: true,
    })
    adoptSession(data)
    await router.replace(returnPath(route.query.next))
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
      <Alert v-if="problem" variant="destructive" role="alert">
        <CircleAlert aria-hidden="true" />
        <AlertDescription>{{ problem }}</AlertDescription>
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
    </form>
  </AuthCard>
</template>
