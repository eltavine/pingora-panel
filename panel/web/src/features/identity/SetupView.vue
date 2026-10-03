<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { CircleAlert, ShieldCheck } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { login, setup, setupStatus } from '@/api/generated'
import FormField from '@/components/FormField.vue'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Spinner } from '@/components/ui/spinner'
import { adoptSession } from '@/lib/session'
import AuthCard from './AuthCard.vue'
import { fieldProblems, signInProblem } from './failures'
import PasswordInput from './PasswordInput.vue'

const { t } = useI18n()
const router = useRouter()
const token = ref('')
const username = ref('')
const displayName = ref('')
const password = ref('')
const confirmation = ref('')
const busy = ref(false)
const problem = ref<string>()
const passwordProblems = ref<string[]>([])
const mismatch = computed(() => confirmation.value !== '' && confirmation.value !== password.value)

onMounted(async () => {
  const { data } = await setupStatus()
  if (data && !data.required) {
    await router.replace({ name: 'login' })
  }
})

async function submit() {
  if (mismatch.value) {
    return
  }
  busy.value = true
  problem.value = undefined
  passwordProblems.value = []
  try {
    await setup({
      body: {
        token: token.value.trim(),
        username: username.value,
        password: password.value,
        display_name: displayName.value.trim() || null,
      },
      throwOnError: true,
    })
    const { data } = await login({
      body: { username: username.value, password: password.value, transport: 'cookie' },
      throwOnError: true,
    })
    adoptSession(data)
    await router.replace('/')
  } catch (error) {
    passwordProblems.value = fieldProblems(error, 'password')
    problem.value = passwordProblems.value.length ? undefined : signInProblem(error, t)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <AuthCard :title="t('auth.setupTitle')" :description="t('auth.setupDescription')">
    <form class="flex flex-col gap-4" @submit.prevent="submit">
      <Alert v-if="problem" variant="destructive" role="alert">
        <CircleAlert aria-hidden="true" />
        <AlertDescription>{{ problem }}</AlertDescription>
      </Alert>
      <FormField id="setup-token" :label="t('auth.bootstrapToken')" :hint="t('auth.bootstrapHint')">
        <PasswordInput
          id="setup-token"
          v-model="token"
          autocomplete="off"
          described-by="setup-token-hint"
        />
      </FormField>
      <FormField id="setup-username" :label="t('auth.username')">
        <Input
          id="setup-username"
          v-model="username"
          autocomplete="username"
          autocapitalize="none"
          spellcheck="false"
          required
        />
      </FormField>
      <FormField id="setup-display-name" :label="t('auth.displayName')">
        <Input id="setup-display-name" v-model="displayName" autocomplete="name" />
      </FormField>
      <FormField id="setup-password" :label="t('auth.newPassword')" :hint="t('auth.passwordHint')">
        <PasswordInput
          id="setup-password"
          v-model="password"
          autocomplete="new-password"
          :invalid="passwordProblems.length > 0"
          described-by="setup-password-hint setup-password-problems"
        />
        <ul
          v-if="passwordProblems.length"
          id="setup-password-problems"
          class="text-destructive flex flex-col gap-0.5 text-xs"
          role="alert"
        >
          <li v-for="item in passwordProblems" :key="item">{{ item }}</li>
        </ul>
      </FormField>
      <FormField id="setup-confirm" :label="t('auth.confirmPassword')">
        <PasswordInput
          id="setup-confirm"
          v-model="confirmation"
          autocomplete="new-password"
          :invalid="mismatch"
        />
        <p v-if="mismatch" class="text-destructive text-xs" role="alert">
          {{ t('auth.mismatch') }}
        </p>
      </FormField>
      <Button type="submit" :disabled="busy || mismatch || !token || !username || !password">
        <Spinner v-if="busy" data-icon="inline-start" />
        <ShieldCheck v-else data-icon="inline-start" aria-hidden="true" />
        {{ t('auth.createAdmin') }}
      </Button>
    </form>
  </AuthCard>
</template>
