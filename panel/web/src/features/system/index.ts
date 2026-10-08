import { MonitorCog } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const systemFeature: FeatureModule = {
  id: 'system',
  group: 'administer',
  permission: 'platform.read',
  routes: [
    {
      path: 'system',
      name: 'system',
      component: () => import('./SystemView.vue'),
      meta: { title: 'nav.system' },
    },
  ],
  navigation: [{ id: 'system', title: 'nav.system', icon: MonitorCog, to: '/system' }],
  messages: {
    'zh-CN': () => import('./locales/zh-CN'),
    en: () => import('./locales/en'),
  },
}
