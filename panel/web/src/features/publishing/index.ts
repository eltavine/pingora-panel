import { Rocket } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const publishingFeature: FeatureModule = {
  id: 'publishing',
  group: 'operate',
  permission: 'gateway.publish',
  routes: [
    {
      path: 'publish',
      name: 'publish',
      component: () => import('./PublishView.vue'),
      meta: { title: 'nav.publish' },
    },
  ],
  navigation: [{ id: 'publish', title: 'nav.publish', icon: Rocket, to: '/publish' }],
}
