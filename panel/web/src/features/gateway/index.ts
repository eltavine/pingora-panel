import { Gauge } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const gatewayFeature: FeatureModule = {
  id: 'gateway',
  group: 'operate',
  permission: 'gateway.read',
  routes: [
    {
      path: '',
      name: 'gateway-overview',
      component: () => import('./GatewayOverview.vue'),
      meta: { title: 'nav.overview' },
    },
  ],
  navigation: [
    {
      id: 'gateway-overview',
      title: 'nav.overview',
      icon: Gauge,
      to: '/',
      navigationBar: 10,
      shortTitle: 'nav.overviewShort',
    },
  ],
}
