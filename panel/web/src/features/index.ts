import { gatewayFeature } from './gateway'
import { publishingFeature } from './publishing'
import { receiptsFeature } from './receipts'
import { sitesFeature } from './sites'
import type { FeatureModule, NavigationGroup } from './types'

/** Every console feature, in navigation order. */
export const features: readonly FeatureModule[] = [
  gatewayFeature,
  publishingFeature,
  receiptsFeature,
  sitesFeature,
]

export const navigationGroups: readonly { id: NavigationGroup; title: string }[] = [
  { id: 'operate', title: 'nav.operate' },
  { id: 'configure', title: 'nav.configure' },
]
