import './styles/main.css'

import { createApp } from 'vue'
import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import App from './App.vue'
import { i18n } from './i18n'
import { configureApi } from './lib/api'
import router from './router'

configureApi()

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 5_000, retry: 1, refetchOnWindowFocus: true },
    // Mutations carry idempotency keys; retries are explicit user actions.
    mutations: { retry: false },
  },
})

createApp(App).use(router).use(i18n).use(VueQueryPlugin, { queryClient }).mount('#app')
