import { ReceiptText } from '@lucide/vue'
import type { FeatureModule } from '@/features/types'

export const receiptsFeature: FeatureModule = {
  id: 'receipts',
  group: 'operate',
  permission: 'gateway.read',
  routes: [
    {
      path: 'receipts',
      name: 'receipts',
      component: () => import('./ReceiptsView.vue'),
      meta: { title: 'nav.receipts' },
    },
  ],
  navigation: [{ id: 'receipts', title: 'nav.receipts', icon: ReceiptText, to: '/receipts' }],
  messages: {
    'zh-CN': () => import('./locales/zh-CN'),
    en: () => import('./locales/en'),
  },
}
