import { Logs } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const logsFeature: FeatureModule = {
  id: 'logs',
  group: 'operate',
  permission: 'logs.read',
  routes: [
    {
      path: 'logs',
      name: 'logs',
      component: () => import('./LogsView.vue'),
      meta: { title: 'nav.logs' },
    },
  ],
  navigation: [{ id: 'logs', title: 'nav.logs', icon: Logs, to: '/logs' }],
}
