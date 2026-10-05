import { DatabaseBackup } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const backupsFeature: FeatureModule = {
  id: 'backups',
  group: 'administer',
  permission: 'backups.read',
  routes: [
    {
      path: 'backups',
      name: 'backups',
      component: () => import('./BackupsView.vue'),
      meta: { title: 'nav.backups' },
    },
  ],
  navigation: [{ id: 'backups', title: 'nav.backups', icon: DatabaseBackup, to: '/backups' }],
}
