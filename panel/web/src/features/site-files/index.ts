import { FolderOpen } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const siteFilesFeature: FeatureModule = {
  id: 'site-files',
  group: 'configure',
  permission: 'files.read',
  routes: [
    {
      path: 'site-files',
      name: 'site-files',
      component: () => import('./SiteFilesView.vue'),
      meta: { title: 'nav.siteFiles' },
    },
  ],
  navigation: [{ id: 'site-files', title: 'nav.siteFiles', icon: FolderOpen, to: '/site-files' }],
  messages: {
    'zh-CN': () => import('./locales/zh-CN'),
    en: () => import('./locales/en'),
  },
}
