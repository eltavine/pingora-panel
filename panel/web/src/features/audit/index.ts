import { ScrollText } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const auditFeature: FeatureModule = {
  id: 'audit',
  group: 'operate',
  permission: 'audit.read',
  routes: [
    {
      path: 'audit',
      name: 'audit',
      component: () => import('./AuditView.vue'),
      meta: { title: 'nav.audit' },
    },
  ],
  navigation: [{ id: 'audit', title: 'nav.audit', icon: ScrollText, to: '/audit' }],
}
