import { Container } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const containersFeature: FeatureModule = {
  id: 'containers',
  group: 'operate',
  permission: 'containers.read',
  routes: [
    {
      path: 'containers',
      name: 'containers',
      component: () => import('./ContainersView.vue'),
      meta: { title: 'nav.containers' },
    },
  ],
  navigation: [{ id: 'containers', title: 'nav.containers', icon: Container, to: '/containers' }],
}
