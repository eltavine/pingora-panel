import { ArrowLeftRight } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const httpPoliciesFeature: FeatureModule = {
  id: 'http-policies',
  group: 'configure',
  permission: 'config.read',
  routes: [
    {
      path: 'http-policies',
      name: 'http-policies',
      component: () => import('./HttpPoliciesView.vue'),
      meta: { title: 'nav.httpPolicies' },
    },
  ],
  navigation: [
    {
      id: 'http-policies',
      title: 'nav.httpPolicies',
      icon: ArrowLeftRight,
      to: '/http-policies',
    },
  ],
}
