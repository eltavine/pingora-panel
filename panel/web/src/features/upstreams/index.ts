import { Server } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const upstreamsFeature: FeatureModule = {
  id: 'upstreams',
  group: 'configure',
  permission: 'config.read',
  routes: [
    {
      path: 'upstreams',
      name: 'upstreams',
      component: () => import('./UpstreamsView.vue'),
      meta: { title: 'nav.upstreams' },
    },
    {
      path: 'upstreams/:id',
      name: 'upstream',
      component: () => import('./UpstreamDetailView.vue'),
      props: true,
      meta: { title: 'nav.upstreams' },
    },
  ],
  navigation: [
    { id: 'upstreams', title: 'nav.upstreams', icon: Server, to: '/upstreams', navigationBar: 50 },
  ],
}
