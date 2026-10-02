import { createRouter, createWebHistory } from 'vue-router'
import AppShell from '@/app/AppShell.vue'
import { features } from '@/features'

const router = createRouter({
  history: createWebHistory(import.meta.env.BASE_URL),
  routes: [
    {
      path: '/',
      component: AppShell,
      children: features.flatMap((feature) => feature.routes),
    },
    { path: '/:pathMatch(.*)*', redirect: '/' },
  ],
})

export default router
