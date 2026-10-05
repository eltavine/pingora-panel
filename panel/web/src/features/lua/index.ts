import { Braces } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const luaFeature: FeatureModule = {
  id: 'lua',
  group: 'configure',
  permission: 'config.read',
  routes: [
    {
      path: 'lua',
      name: 'lua',
      component: () => import('./LuaView.vue'),
      meta: { title: 'nav.lua' },
    },
  ],
  navigation: [{ id: 'lua', title: 'nav.lua', icon: Braces, to: '/lua' }],
}
