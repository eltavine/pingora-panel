import { FileBadge } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const certificatesFeature: FeatureModule = {
  id: 'certificates',
  group: 'configure',
  permission: 'certificate.read',
  routes: [
    {
      path: 'certificates',
      name: 'certificates',
      component: () => import('./CertificatesView.vue'),
      meta: { title: 'nav.certificates' },
    },
  ],
  navigation: [
    { id: 'certificates', title: 'nav.certificates', icon: FileBadge, to: '/certificates' },
  ],
}
