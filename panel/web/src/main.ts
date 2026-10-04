import './styles/main.css'

import { createApp } from 'vue'
import { VueQueryPlugin } from '@tanstack/vue-query'
import App from './App.vue'
import { i18n, setLocale } from './i18n'
import { configureApi } from './lib/api'
import { queryClient } from './lib/query'
import { installSession } from './lib/session'
import router, { loginFor } from './router'

async function start() {
  const messages = setLocale(i18n.global.locale.value)
  if (import.meta.env.MODE === 'mock') {
    const { startMocks } = await import('./mocks')
    await startMocks()
  }
  configureApi()
  installSession(() => {
    const current = router.currentRoute.value
    if (!current.meta.public) {
      void router.replace(loginFor(current.fullPath))
    }
  })
  await messages
  createApp(App).use(router).use(i18n).use(VueQueryPlugin, { queryClient }).mount('#app')
}

void start()
