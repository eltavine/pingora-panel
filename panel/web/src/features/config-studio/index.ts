import { FileCode2 } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const configStudioFeature: FeatureModule = {
  id: 'config-studio',
  group: 'configure',
  routes: [
    {
      path: 'config',
      name: 'config',
      component: () => import('./ConfigStudioView.vue'),
      meta: { title: 'nav.config' },
    },
  ],
  navigation: [{ id: 'config', title: 'nav.config', icon: FileCode2, to: '/config' }],
}
