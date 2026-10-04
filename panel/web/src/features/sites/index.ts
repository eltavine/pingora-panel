import { Globe } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const sitesFeature: FeatureModule = {
  id: 'sites',
  group: 'configure',
  permission: 'config.read',
  routes: [
    {
      path: 'sites',
      name: 'sites',
      component: () => import('./SitesView.vue'),
      meta: { title: 'nav.sites' },
    },
    {
      path: 'sites/:id',
      name: 'site',
      component: () => import('./SiteDetailView.vue'),
      props: true,
      meta: { title: 'nav.sites' },
    },
  ],
  navigation: [{ id: 'sites', title: 'nav.sites', icon: Globe, to: '/sites', navigationBar: 30 }],
}
