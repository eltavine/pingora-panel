import { DatabaseZap } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const cacheFeature: FeatureModule = {
  id: 'cache',
  group: 'configure',
  permission: 'config.read',
  routes: [
    {
      path: 'cache',
      name: 'cache',
      component: () => import('./CacheView.vue'),
      meta: { title: 'nav.cache' },
    },
  ],
  navigation: [
    {
      id: 'cache',
      title: 'nav.cache',
      icon: DatabaseZap,
      to: '/cache',
    },
  ],
  messages: {
    'zh-CN': () => import('./locales/zh-CN'),
    en: () => import('./locales/en'),
  },
}
