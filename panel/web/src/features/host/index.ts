import { ServerCog } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const hostFeature: FeatureModule = {
  id: 'host',
  group: 'operate',
  permission: 'host.read',
  routes: [
    {
      path: 'host',
      name: 'host',
      component: () => import('./HostView.vue'),
      meta: { title: 'nav.host' },
    },
  ],
  navigation: [{ id: 'host', title: 'nav.host', icon: ServerCog, to: '/host' }],
}
