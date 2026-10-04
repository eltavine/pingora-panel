import { Stamp } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const approvalsFeature: FeatureModule = {
  id: 'approvals',
  group: 'operate',
  permission: 'config.read',
  routes: [
    {
      path: 'approvals',
      name: 'approvals',
      component: () => import('./ApprovalsView.vue'),
      meta: { title: 'nav.approvals' },
    },
  ],
  navigation: [
    { id: 'approvals', title: 'nav.approvals', icon: Stamp, to: '/approvals', navigationBar: 40 },
  ],
}
