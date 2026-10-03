import { Radio } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const listenersFeature: FeatureModule = {
  id: 'listeners',
  group: 'configure',
  routes: [
    {
      path: 'listeners',
      name: 'listeners',
      component: () => import('./ListenersView.vue'),
      meta: { title: 'nav.listeners' },
    },
  ],
  navigation: [{ id: 'listeners', title: 'nav.listeners', icon: Radio, to: '/listeners' }],
}
