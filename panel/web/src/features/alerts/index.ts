import { BellRing } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const alertsFeature: FeatureModule = {
  id: 'alerts',
  group: 'operate',
  permission: 'alerts.read',
  routes: [
    {
      path: 'alerts',
      name: 'alerts',
      component: () => import('./AlertsView.vue'),
      meta: { title: 'nav.alerts' },
    },
  ],
  navigation: [{ id: 'alerts', title: 'nav.alerts', icon: BellRing, to: '/alerts' }],
  messages: {
    'zh-CN': () => import('./locales/zh-CN'),
    en: () => import('./locales/en'),
  },
}
