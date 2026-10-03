import './styles/main.css'

import { createApp } from 'vue'
import { VueQueryPlugin } from '@tanstack/vue-query'
import App from './App.vue'
import { i18n } from './i18n'
import { configureApi } from './lib/api'
import { queryClient } from './lib/query'
import { installSession } from './lib/session'
import router, { loginFor } from './router'

configureApi()
installSession(() => {
  const current = router.currentRoute.value
  if (!current.meta.public) {
    void router.replace(loginFor(current.fullPath))
  }
})

createApp(App).use(router).use(i18n).use(VueQueryPlugin, { queryClient }).mount('#app')
