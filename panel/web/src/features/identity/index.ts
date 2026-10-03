import { Shield, Users } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const identityFeature: FeatureModule = {
  id: 'identity',
  group: 'administer',
  permission: 'identity.read',
  routes: [
    {
      path: 'account',
      name: 'account',
      component: () => import('./AccountSettingsView.vue'),
      // Every account may manage its own password, sessions and tokens.
      meta: { title: 'nav.account', permission: undefined },
    },
    {
      path: 'accounts',
      name: 'accounts',
      component: () => import('./AccountsView.vue'),
      meta: { title: 'nav.accounts' },
    },
    {
      path: 'roles',
      name: 'roles',
      component: () => import('./RolesView.vue'),
      meta: { title: 'nav.roles' },
    },
  ],
  navigation: [
    { id: 'accounts', title: 'nav.accounts', icon: Users, to: '/accounts' },
    { id: 'roles', title: 'nav.roles', icon: Shield, to: '/roles' },
  ],
}
