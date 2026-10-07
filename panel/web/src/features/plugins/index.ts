import { Puzzle } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const pluginsFeature: FeatureModule = {
  id: 'plugins',
  group: 'administer',
  permission: 'plugins.read',
  routes: [
    {
      path: 'plugins',
      name: 'plugins',
      component: () => import('./PluginsView.vue'),
      meta: { title: 'nav.plugins' },
    },
    {
      path: 'plugins/:name',
      name: 'plugin',
      component: () => import('./PluginView.vue'),
      meta: { title: 'nav.plugins' },
    },
  ],
  navigation: [{ id: 'plugins', title: 'nav.plugins', icon: Puzzle, to: '/plugins' }],
  messages: {
    'zh-CN': () => import('./locales/zh-CN'),
    en: () => import('./locales/en'),
  },
}
