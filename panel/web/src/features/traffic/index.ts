import { Activity } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const trafficFeature: FeatureModule = {
  id: 'traffic',
  group: 'operate',
  permission: 'gateway.read',
  routes: [
    {
      path: 'traffic',
      name: 'traffic',
      component: () => import('./TrafficView.vue'),
      meta: { title: 'nav.traffic' },
    },
  ],
  navigation: [{ id: 'traffic', title: 'nav.traffic', icon: Activity, to: '/traffic' }],
}
