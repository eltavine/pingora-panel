import { History } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const revisionsFeature: FeatureModule = {
  id: 'revisions',
  group: 'operate',
  permission: 'config.read',
  routes: [
    {
      path: 'revisions',
      name: 'revisions',
      component: () => import('./RevisionsView.vue'),
      meta: { title: 'nav.revisions' },
    },
    {
      path: 'revisions/:id(\\d+)',
      name: 'revision',
      component: () => import('./RevisionDetailView.vue'),
      props: true,
      meta: { title: 'nav.revisions' },
    },
  ],
  navigation: [{ id: 'revisions', title: 'nav.revisions', icon: History, to: '/revisions' }],
  messages: {
    'zh-CN': () => import('./locales/zh-CN'),
    en: () => import('./locales/en'),
  },
}
