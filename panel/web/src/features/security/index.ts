import { ShieldBan } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const securityFeature: FeatureModule = {
  id: 'security',
  group: 'configure',
  permission: 'config.read',
  routes: [
    {
      path: 'security-policies',
      name: 'security-policies',
      component: () => import('./SecurityPoliciesView.vue'),
      meta: { title: 'nav.securityPolicies' },
    },
  ],
  navigation: [
    {
      id: 'security-policies',
      title: 'nav.securityPolicies',
      icon: ShieldBan,
      to: '/security-policies',
    },
  ],
}
