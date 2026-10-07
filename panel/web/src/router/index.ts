import { createRouter, createWebHistory, type RouteLocationRaw } from 'vue-router'
import { setupStatus } from '@/api/generated'
import AppShell from '@/app/AppShell.vue'
import { features } from '@/features'
import { loadFeatureMessages, type FeatureMessages } from '@/i18n'
import { loadSession } from '@/lib/session'

declare module 'vue-router' {
  interface RouteMeta {
    /** i18n key of the page title. */
    title?: string
    /** Reachable without logging in. */
    public?: boolean
    /** The API permission the page needs. */
    permission?: string
    /** Messages of the feature the page belongs to. */
    messages?: FeatureMessages
  }
}

/** Where an account goes when it may not see the page it asked for. */
const ACCOUNT_PAGE = '/account'

const router = createRouter({
  history: createWebHistory(import.meta.env.BASE_URL),
  routes: [
    {
      path: '/login',
      name: 'login',
      component: () => import('@/features/identity/LoginView.vue'),
      meta: { public: true, title: 'auth.login' },
    },
    {
      path: '/setup',
      name: 'setup',
      component: () => import('@/features/identity/SetupView.vue'),
      meta: { public: true, title: 'auth.setup' },
    },
    {
      path: '/',
      component: AppShell,
      children: features.flatMap((feature) =>
        feature.routes.map((route) => ({
          ...route,
          meta: { permission: feature.permission, messages: feature.messages, ...route.meta },
        })),
      ),
    },
    { path: '/:pathMatch(.*)*', redirect: '/' },
  ],
})

/** The login page, returning to `target` afterwards. */
export function loginFor(target: string): RouteLocationRaw {
  return target === '/' || target.startsWith('/login')
    ? { name: 'login' }
    : { name: 'login', query: { next: target } }
}

router.beforeEach(async (to) => {
  if (to.meta.public) {
    return true
  }
  let session
  try {
    session = await loadSession()
  } catch {
    // The API is unreachable; the page reports it.
    return true
  }
  if (!session) {
    const { data } = await setupStatus()
    return data?.required ? { name: 'setup' } : loginFor(to.fullPath)
  }
  const permission = to.meta.permission
  if (permission && !session.permissions.includes(permission) && to.path !== ACCOUNT_PAGE) {
    return ACCOUNT_PAGE
  }
  return true
})

router.beforeResolve(async (to) => {
  await Promise.all(to.matched.map((record) => loadFeatureMessages(record.meta.messages)))
})

export default router
